//! Importers for foreign containers: notebook-app backup archives, single `.goodnotes` notebooks and
//! Word (`.docx`) documents.
//!
//! The `.goodnotes` layout (a ZIP with `attachments/<UUID>` binaries without extensions,
//! `notes/<page>` stroke blobs, `index.*.pb` indexes and `thumbnail.jpg`, and backups being ZIPs of
//! such files in a folder tree) was learned from the notes of an MIT-licensed open-source converter (see ATTRIBUTION.md).
//! No code was copied. Stroke blobs are proprietary and are **not** decoded; background PDFs,
//! embedded pictures and the preview image are.
//!
//! Every input is hostile: entry counts, entry sizes and total unpacked bytes are capped and
//! nothing here panics.

use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Largest archive accepted (bytes). Larger files are refused instead of exhausting browser memory.
pub const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_NOTEBOOKS: usize = 1_000;
const MAX_FOLDER_DEPTH: usize = 8;
const MAX_SEGMENT_CHARS: usize = 64;
const MAX_DOCX_BLOCKS: usize = 20_000;
const MAX_DOCX_IMAGES: usize = 100;
const MAX_DOCX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MIN_PICTURE_BYTES: usize = 2 * 1024;

/// Why an import failed.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("this file is not a readable archive")]
    NotAnArchive,
    #[error("the archive is too large to import in this browser ({0} MB max)")]
    TooLarge(usize),
    #[error("nothing importable was found in this file")]
    Empty,
}

/// What kind of container a byte buffer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    /// A ZIP of `.goodnotes` files or of unpacked notebook folders.
    NotebookBackup,
    /// One `.goodnotes` notebook.
    SingleNotebook,
    /// A Word document.
    Docx,
    /// Anything else.
    Other,
}

/// One notebook found in a backup archive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportedNotebook {
    /// Notebook title (file or folder name without extension).
    pub name: String,
    /// Sanitized folder path inside the backup, `""` for the root.
    pub folder_path: String,
    /// The embedded background PDF (the largest one when there are several).
    pub pdf: Option<Vec<u8>>,
    /// Embedded pictures (PNG / JPEG), used when there is no PDF.
    pub images: Vec<Vec<u8>>,
    /// `thumbnail.jpg` preview of the first page.
    pub thumbnail: Option<Vec<u8>>,
}

/// A block of a Word document in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocxBlock {
    /// `heading` is `0` for body text and `1..=3` for Title / Heading 1-3.
    Paragraph {
        text: String,
        heading: u8,
    },
    Image(Vec<u8>),
}

fn open(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, ImportError> {
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(ImportError::TooLarge(MAX_ARCHIVE_BYTES / (1024 * 1024)));
    }
    if !bytes.starts_with(b"PK") {
        return Err(ImportError::NotAnArchive);
    }
    let archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| ImportError::NotAnArchive)?;
    if archive.len() > MAX_ENTRIES {
        return Err(ImportError::TooLarge(MAX_ARCHIVE_BYTES / (1024 * 1024)));
    }
    Ok(archive)
}

fn entry_names<R: std::io::Read + std::io::Seek>(archive: &ZipArchive<R>) -> Vec<String> {
    archive
        .file_names()
        .filter_map(|name| name.ok().map(std::borrow::Cow::into_owned))
        .collect()
}

/// Classifies `bytes` by looking at entry names only (cheap: no entry is unpacked).
#[must_use]
pub fn sniff_archive(bytes: &[u8]) -> ArchiveKind {
    let Ok(archive) = open(bytes) else {
        return ArchiveKind::Other;
    };
    let names = entry_names(&archive);
    let mut has_notebook_marker = false;
    for name in &names {
        let lower = name.to_ascii_lowercase();
        if lower == "word/document.xml" {
            return ArchiveKind::Docx;
        }
        if lower.ends_with(".goodnotes") {
            return ArchiveKind::NotebookBackup;
        }
        if lower.ends_with("index.notes.pb") || lower.contains("/attachments/") {
            has_notebook_marker = true;
        }
        if lower.starts_with("attachments/") || lower.starts_with("notes/") {
            has_notebook_marker = true;
        }
    }
    if has_notebook_marker {
        // Unpacked folders (`<Name>/attachments/...`) are read like a backup; a root-level
        // notebook is a single `.goodnotes` file.
        let nested = names
            .iter()
            .any(|n| n.contains('/') && !n.starts_with("attachments/") && !n.starts_with("notes/"));
        if nested {
            ArchiveKind::NotebookBackup
        } else {
            ArchiveKind::SingleNotebook
        }
    } else {
        ArchiveKind::Other
    }
}

fn clean_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(MAX_SEGMENT_CHARS)
        .collect();
    cleaned.trim().to_string()
}

/// Turns the folder part of an archive path into a safe library folder path.
fn clean_folder(path: &str) -> String {
    let segments: Vec<String> = path
        .split('/')
        .map(clean_segment)
        .filter(|s| !s.is_empty() && s != "." && s != "..")
        .take(MAX_FOLDER_DEPTH)
        .collect();
    segments.join("/")
}

fn file_stem(name: &str) -> String {
    let leaf = name.rsplit('/').next().unwrap_or(name);
    let stem = leaf.rsplit_once('.').map_or(leaf, |(s, _)| s);
    let cleaned = clean_segment(stem);
    if cleaned.is_empty() {
        "Imported notebook".to_string()
    } else {
        cleaned
    }
}

fn read_limited<R: Read>(reader: R, limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut limited = reader.take(limit.saturating_add(1));
    limited.read_to_end(&mut out).ok()?;
    if out.len() as u64 > limit {
        return None;
    }
    Some(out)
}

enum Picture {
    Pdf,
    Raster,
}

fn classify(bytes: &[u8]) -> Option<Picture> {
    if bytes.starts_with(b"%PDF-") {
        Some(Picture::Pdf)
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
    {
        Some(Picture::Raster)
    } else {
        None
    }
}

/// Collects one notebook from `archive`, looking only at entries below `prefix` (`""` = root).
fn collect_notebook<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    prefix: &str,
    name: String,
    folder_path: String,
    budget: &mut u64,
) -> Option<ImportedNotebook> {
    let mut nb = ImportedNotebook {
        name,
        folder_path,
        ..ImportedNotebook::default()
    };
    for idx in 0..archive.len() {
        let Ok(file) = archive.by_index(idx) else {
            continue;
        };
        if file.is_dir() {
            continue;
        }
        let Ok(full) = file.name().map(std::borrow::Cow::into_owned) else {
            continue;
        };
        let Some(rel) = full.strip_prefix(prefix) else {
            continue;
        };
        let is_attachment = rel.starts_with("attachments/");
        let is_thumb = rel.eq_ignore_ascii_case("thumbnail.jpg");
        if !is_attachment && !is_thumb {
            continue;
        }
        if file.size() > MAX_ENTRY_BYTES || file.size() > *budget {
            continue;
        }
        let Some(data) = read_limited(file, MAX_ENTRY_BYTES) else {
            continue;
        };
        *budget = budget.saturating_sub(data.len() as u64);
        if is_thumb {
            nb.thumbnail = Some(data);
            continue;
        }
        match classify(&data) {
            Some(Picture::Pdf) => {
                if nb.pdf.as_ref().is_none_or(|old| data.len() > old.len()) {
                    nb.pdf = Some(data);
                }
            }
            Some(Picture::Raster) if data.len() >= MIN_PICTURE_BYTES => nb.images.push(data),
            _ => {}
        }
    }
    (nb.pdf.is_some() || !nb.images.is_empty() || nb.thumbnail.is_some()).then_some(nb)
}

/// Reads every notebook of a notebook-app backup, a folder-style backup or a single `.goodnotes`
/// file. `fallback_name` titles a root-level notebook (usually the picked file's name).
pub fn read_notebook_archive(
    bytes: &[u8],
    fallback_name: &str,
) -> Result<Vec<ImportedNotebook>, ImportError> {
    let mut archive = open(bytes)?;
    let mut budget = MAX_TOTAL_UNPACKED;
    let mut found = Vec::new();

    let names = entry_names(&archive);
    let nested: Vec<&String> = names
        .iter()
        .filter(|n| n.to_ascii_lowercase().ends_with(".goodnotes"))
        .collect();

    if !nested.is_empty() {
        for entry_name in nested.into_iter().take(MAX_NOTEBOOKS) {
            let Ok(file) = archive.by_name(entry_name) else {
                continue;
            };
            if file.size() > MAX_ENTRY_BYTES || file.size() > budget {
                continue;
            }
            let Some(inner_bytes) = read_limited(file, MAX_ENTRY_BYTES) else {
                continue;
            };
            let Ok(mut inner) = open(&inner_bytes) else {
                continue;
            };
            let (folder, leaf) = entry_name.rsplit_once('/').unwrap_or(("", entry_name));
            if let Some(nb) = collect_notebook(
                &mut inner,
                "",
                file_stem(leaf),
                clean_folder(folder),
                &mut budget,
            ) {
                found.push(nb);
            }
        }
    } else {
        // Unpacked folders: every `<Name>/attachments/...` group is a notebook.
        let mut roots: Vec<String> = Vec::new();
        for name in &names {
            let Some(pos) = name
                .find("/attachments/")
                .or_else(|| name.find("/index.notes.pb"))
            else {
                continue;
            };
            let root = &name[..pos];
            if !roots.iter().any(|r| r == root) {
                roots.push(root.to_string());
            }
        }
        if roots.is_empty() {
            let title = file_stem(fallback_name);
            if let Some(nb) = collect_notebook(&mut archive, "", title, String::new(), &mut budget)
            {
                found.push(nb);
            }
        }
        for root in roots.into_iter().take(MAX_NOTEBOOKS) {
            let (folder, leaf) = root.rsplit_once('/').unwrap_or(("", root.as_str()));
            let prefix = format!("{root}/");
            if let Some(nb) = collect_notebook(
                &mut archive,
                &prefix,
                clean_segment(leaf),
                clean_folder(folder),
                &mut budget,
            ) {
                found.push(nb);
            }
        }
    }
    if found.is_empty() {
        Err(ImportError::Empty)
    } else {
        Ok(found)
    }
}

// ---------------------------------------------------------------------------------------------
// DOCX
// ---------------------------------------------------------------------------------------------

fn decode_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix('#')
                .and_then(|num| {
                    num.strip_prefix(['x', 'X']).map_or_else(
                        || num.parse::<u32>().ok(),
                        |hex| u32::from_str_radix(hex, 16).ok(),
                    )
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => out.push(ch),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn read_zip_text<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let file = archive.by_name(name).ok()?;
    let data = read_limited(file, 64 * 1024 * 1024)?;
    String::from_utf8(data).ok()
}

/// Reads the text and pictures of a `.docx` in reading order.
pub fn read_docx(bytes: &[u8]) -> Result<Vec<DocxBlock>, ImportError> {
    let mut archive = open(bytes)?;
    let document = read_zip_text(&mut archive, "word/document.xml").ok_or(ImportError::Empty)?;
    let rels = read_zip_text(&mut archive, "word/_rels/document.xml.rels").unwrap_or_default();

    // relationship id -> media path
    let mut targets: Vec<(String, String)> = Vec::new();
    for tag in rels.split('<').filter(|t| t.starts_with("Relationship ")) {
        if let (Some(id), Some(target)) = (attr(tag, "Id"), attr(tag, "Target")) {
            targets.push((id.to_string(), target.to_string()));
        }
    }

    let mut blocks: Vec<DocxBlock> = Vec::new();
    let mut text = String::new();
    let mut heading = 0u8;
    let mut pending_images: Vec<String> = Vec::new();
    let mut images_read = 0usize;
    let mut rest = document.as_str();

    while let Some(lt) = rest.find('<') {
        let after = &rest[lt + 1..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let tag = &after[..gt];
        rest = &after[gt + 1..];
        let closing = tag.starts_with('/');
        let name = tag
            .strip_prefix('/')
            .unwrap_or(tag)
            .split(['/', ' ', '\t', '\n', '\r'])
            .next()
            .unwrap_or("");
        match (closing, name) {
            (false, "w:p") => {
                text.clear();
                heading = 0;
                pending_images.clear();
            }
            (false, "w:pStyle") => {
                if let Some(style) = attr(tag, "w:val") {
                    let lower = style.to_ascii_lowercase();
                    heading = if lower == "title" {
                        1
                    } else if let Some(level) = lower.strip_prefix("heading") {
                        level.parse::<u8>().ok().map_or(0, |l| l.clamp(1, 3))
                    } else {
                        0
                    };
                }
            }
            (false, "w:t") => {
                if !tag.ends_with('/')
                    && let Some(end) = rest.find("</w:t>")
                {
                    text.push_str(&decode_entities(&rest[..end]));
                    rest = &rest[end..];
                }
            }
            (false, "w:tab") => text.push('\t'),
            (false, "w:br" | "w:cr") => text.push('\n'),
            (false, "a:blip") => {
                if let Some(id) = attr(tag, "r:embed") {
                    pending_images.push(id.to_string());
                }
            }
            (true, "w:p") => {
                if !text.trim().is_empty() {
                    blocks.push(DocxBlock::Paragraph {
                        text: text.trim_end().to_string(),
                        heading,
                    });
                }
                for id in pending_images.drain(..) {
                    if images_read >= MAX_DOCX_IMAGES {
                        break;
                    }
                    let Some((_, target)) = targets.iter().find(|(rid, _)| *rid == id) else {
                        continue;
                    };
                    let path = if let Some(abs) = target.strip_prefix('/') {
                        abs.to_string()
                    } else {
                        format!("word/{target}")
                    };
                    let Ok(file) = archive.by_name(&path) else {
                        continue;
                    };
                    if let Some(data) = read_limited(file, MAX_DOCX_IMAGE_BYTES)
                        && matches!(classify(&data), Some(Picture::Raster))
                    {
                        images_read += 1;
                        blocks.push(DocxBlock::Image(data));
                    }
                }
                text.clear();
                if blocks.len() >= MAX_DOCX_BLOCKS {
                    break;
                }
            }
            _ => {}
        }
    }
    if blocks.is_empty() {
        Err(ImportError::Empty)
    } else {
        Ok(blocks)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut out);
            for (name, data) in entries {
                w.start_file(*name, SimpleFileOptions::default()).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        out.into_inner()
    }

    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<<>>endobj\n";

    fn sample_notebook_with_pdf() -> Vec<u8> {
        zip_of(&[
            ("index.notes.pb", b"x"),
            ("attachments/AAAA", PDF),
            ("thumbnail.jpg", &[0xFF, 0xD8, 0xFF, 0xE0]),
            ("notes/page", b"strokes"),
        ])
    }

    #[test]
    fn backup_keeps_folder_hierarchy_and_titles() {
        let inner = sample_notebook_with_pdf();
        let backup = zip_of(&[
            (".goodnotesmeta", b"{}"),
            ("School/Physics/Kinematics.goodnotes", &inner),
            ("Whiteboard.goodnotes", &inner),
        ]);
        assert_eq!(sniff_archive(&backup), ArchiveKind::NotebookBackup);
        let books = read_notebook_archive(&backup, "backup.zip").unwrap();
        assert_eq!(books.len(), 2);
        let physics = books.iter().find(|b| b.name == "Kinematics").unwrap();
        assert_eq!(physics.folder_path, "School/Physics");
        assert_eq!(physics.pdf.as_deref(), Some(PDF));
        assert!(physics.thumbnail.is_some());
        let board = books.iter().find(|b| b.name == "Whiteboard").unwrap();
        assert_eq!(board.folder_path, "");
    }

    #[test]
    fn single_notebook_and_unpacked_folders_are_read() {
        let single = sample_notebook_with_pdf();
        assert_eq!(sniff_archive(&single), ArchiveKind::SingleNotebook);
        let books = read_notebook_archive(&single, "My Notes.goodnotes").unwrap();
        assert_eq!(books[0].name, "My Notes");

        let unpacked = zip_of(&[
            ("Math/Algebra/attachments/AAAA", PDF),
            ("Math/Algebra/index.notes.pb", b"x"),
        ]);
        assert_eq!(sniff_archive(&unpacked), ArchiveKind::NotebookBackup);
        let books = read_notebook_archive(&unpacked, "x.zip").unwrap();
        assert_eq!(books[0].name, "Algebra");
        assert_eq!(books[0].folder_path, "Math");
    }

    #[test]
    fn hostile_paths_and_garbage_are_neutralized() {
        let inner = sample_notebook_with_pdf();
        let backup = zip_of(&[("../../etc/./Evil:Name/..\\x.goodnotes", &inner)]);
        let books = read_notebook_archive(&backup, "b.zip").unwrap();
        assert!(!books[0].folder_path.contains(".."));
        assert!(!books[0].name.contains(':'));
        assert!(matches!(
            read_notebook_archive(b"not a zip", "x"),
            Err(ImportError::NotAnArchive)
        ));
        assert!(matches!(
            read_notebook_archive(&zip_of(&[("readme.txt", b"hi")]), "x"),
            Err(ImportError::Empty)
        ));
        assert_eq!(sniff_archive(&[]), ArchiveKind::Other);
        assert_eq!(sniff_archive(b"PK\x03\x04truncated"), ArchiveKind::Other);
    }

    #[test]
    fn docx_text_headings_entities_and_images_in_order() {
        let png: Vec<u8> = [&[0x89, b'P', b'N', b'G'][..], &[0u8; 16][..]].concat();
        let document = br#"<w:document><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Intro &amp; goals</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:t>world &#65;</w:t></w:r></w:p>
<w:p><w:r><w:drawing><a:blip r:embed="rId5"/></w:drawing></w:r></w:p>
</w:body></w:document>"#;
        let rels = br#"<Relationships><Relationship Id="rId5" Type="image" Target="media/image1.png"/></Relationships>"#;
        let docx = zip_of(&[
            ("word/document.xml", document),
            ("word/_rels/document.xml.rels", rels),
            ("word/media/image1.png", &png),
        ]);
        assert_eq!(sniff_archive(&docx), ArchiveKind::Docx);
        let blocks = read_docx(&docx).unwrap();
        assert_eq!(
            blocks[0],
            DocxBlock::Paragraph {
                text: "Intro & goals".to_string(),
                heading: 1
            }
        );
        assert_eq!(
            blocks[1],
            DocxBlock::Paragraph {
                text: "Hello world A".to_string(),
                heading: 0
            }
        );
        assert_eq!(blocks[2], DocxBlock::Image(png));
        assert!(matches!(read_docx(b"junk"), Err(ImportError::NotAnArchive)));
    }

    /// Optional real-data check: set `DRAWCRAFT_IMPORT_BACKUP` to a backup `.zip` to run it.
    #[test]
    fn real_backup_from_environment_imports_when_provided() {
        let Ok(path) = std::env::var("DRAWCRAFT_IMPORT_BACKUP") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(sniff_archive(&bytes), ArchiveKind::NotebookBackup);
        let books = read_notebook_archive(&bytes, "backup.zip").unwrap();
        let with_pdf = books.iter().filter(|b| b.pdf.is_some()).count();
        let with_thumb = books.iter().filter(|b| b.thumbnail.is_some()).count();
        let nested = books.iter().filter(|b| !b.folder_path.is_empty()).count();
        eprintln!(
            "REAL BACKUP: {} notebooks, {with_pdf} with a PDF, {with_thumb} with a preview, {nested} inside folders",
            books.len()
        );
        assert!(!books.is_empty());
    }
}
