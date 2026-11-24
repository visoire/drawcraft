//! Notebook package folder (`<name>.dcraftbook/`): atomic, page-granular persistence with
//! corrupt-file quarantine.
//!
//! Layout:
//! ```text
//! name.dcraftbook/
//!   manifest.json            written LAST, atomically; the only file that makes a save visible
//!   pages/p-<sha16>.json     one content-addressed file per page
//!   assets/a-<sha16>.json    embedded PDFs and image templates
//! ```
//!
//! A save writes only the page and asset files whose content changed, then replaces the manifest.
//! A crash before the manifest rename leaves the previous manifest and every file it references
//! untouched. Unreadable files are never overwritten or deleted: they are renamed to
//! `<file>.corrupt` and the notebook opens with a blank stand-in for the lost page.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use drawcraft_doc::{
    Document, EmbeddedImageTemplate, EmbeddedPdf, Flashcard, NotebookCover, NotebookPage,
    PaperColor, PaperTemplate,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Directory extension for notebook package folders.
pub const PACKAGE_DIR_EXTENSION: &str = "dcraftbook";
/// File name of the manifest.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Maximum pages in one package.
pub const MAX_PACKAGE_PAGES: usize = 20_000;
/// Maximum embedded assets in one package.
pub const MAX_PACKAGE_ASSETS: usize = 256;
/// Largest single file read from a package (hostile-input cap).
pub const MAX_PACKAGE_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MANIFEST_FORMAT: &str = "drawcraft-notebook";
const MANIFEST_VERSION: u32 = 1;

/// Failure to write or open a package folder.
#[derive(Debug)]
pub enum PackageError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The manifest is unreadable; it was moved aside to `quarantined` (if the rename worked).
    ManifestCorrupt {
        quarantined: Option<PathBuf>,
        reason: String,
    },
    UnsupportedVersion(u32),
    Encode(String),
    TooLarge(&'static str),
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::ManifestCorrupt {
                quarantined,
                reason,
            } => match quarantined {
                Some(p) => write!(f, "corrupt manifest ({reason}); moved to {}", p.display()),
                None => write!(f, "corrupt manifest ({reason})"),
            },
            Self::UnsupportedVersion(v) => write!(f, "unsupported package version {v}"),
            Self::Encode(why) => write!(f, "package encoding failed: {why}"),
            Self::TooLarge(what) => write!(f, "package exceeds the {what} limit"),
        }
    }
}

impl std::error::Error for PackageError {}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> PackageError + '_ {
    move |source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Serialize, Deserialize)]
struct FileEntry {
    file: String,
    sha256: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct PageEntry {
    #[serde(flatten)]
    entry: FileEntry,
    width: u32,
    height: u32,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    id: String,
    title: String,
    cover: NotebookCover,
    folder_path: String,
    favorite: bool,
    tags: Vec<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    meta_updated_at_ms: i64,
    next_stroke_id: u64,
    current_page: usize,
    #[serde(default)]
    flashcards: Vec<Flashcard>,
    pages: Vec<PageEntry>,
    #[serde(default)]
    pdfs: Vec<FileEntry>,
    #[serde(default)]
    image_templates: Vec<FileEntry>,
}

/// What a save did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteReport {
    pub pages_written: usize,
    pub pages_reused: usize,
    pub assets_written: usize,
    pub assets_reused: usize,
    pub files_removed: usize,
}

/// A file moved aside because it could not be trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quarantined {
    pub original: PathBuf,
    pub moved_to: PathBuf,
    pub reason: String,
}

/// A package opened from disk, possibly with quarantined parts.
#[derive(Debug)]
pub struct OpenedPackage {
    pub document: Document,
    pub quarantined: Vec<Quarantined>,
    /// 0-based pages replaced by a blank stand-in because their file was lost or corrupt.
    pub blank_pages: Vec<usize>,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Writes `bytes` to `path` atomically: temp file in the same directory, flush, then rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut counter = 0u32;
    let tmp = loop {
        let candidate = parent.join(format!("{name}.tmp.{counter}"));
        if !candidate.exists() {
            break candidate;
        }
        counter += 1;
        if counter > 1_000 {
            return Err(io::Error::other("no free temporary file name"));
        }
    };
    let result = (|| {
        {
            // The handle must be closed before the rename (required on Windows).
            let mut file = fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Moves `path` to `<path>.corrupt` (or `.corrupt.N`), never overwriting an earlier quarantine.
pub fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut target = parent.join(format!("{name}.corrupt"));
    let mut n = 1u32;
    while target.exists() {
        target = parent.join(format!("{name}.corrupt.{n}"));
        n += 1;
        if n > 10_000 {
            return Err(io::Error::other("too many quarantined copies"));
        }
    }
    fs::rename(path, &target)?;
    Ok(target)
}

fn ensure_dir(path: &Path) -> Result<(), PackageError> {
    fs::create_dir_all(path).map_err(io_err(path))
}

/// Writes `content` as a content-addressed file; returns `(entry, newly_written)`.
fn put_content(
    dir: &Path,
    sub: &str,
    prefix: &str,
    content: &[u8],
) -> Result<(FileEntry, bool), PackageError> {
    let sha = sha256_hex(content);
    let rel = format!("{sub}/{prefix}-{}.json", &sha[..16]);
    let path = dir.join(&rel);
    let len = content.len() as u64;
    let up_to_date = fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == len);
    if !up_to_date {
        atomic_write(&path, content).map_err(io_err(&path))?;
    }
    Ok((
        FileEntry {
            file: rel,
            sha256: sha,
            bytes: len,
        },
        !up_to_date,
    ))
}

/// Saves `doc` into the package folder `dir`: changed pages and assets first, manifest last.
pub fn write_package(dir: &Path, doc: &Document) -> Result<WriteReport, PackageError> {
    let pages = doc.resolved_pages();
    if pages.len() > MAX_PACKAGE_PAGES {
        return Err(PackageError::TooLarge("page"));
    }
    if doc.pdf_documents.len() + doc.image_templates.len() > MAX_PACKAGE_ASSETS {
        return Err(PackageError::TooLarge("asset"));
    }
    ensure_dir(&dir.join("pages"))?;
    ensure_dir(&dir.join("assets"))?;
    let mut report = WriteReport::default();

    let mut page_entries = Vec::with_capacity(pages.len());
    for page in &pages {
        let json = serde_json::to_vec(page).map_err(|e| PackageError::Encode(e.to_string()))?;
        let (entry, written) = put_content(dir, "pages", "p", &json)?;
        if written {
            report.pages_written += 1;
        } else {
            report.pages_reused += 1;
        }
        page_entries.push(PageEntry {
            entry,
            width: page.width,
            height: page.height,
        });
    }

    let mut pdfs = Vec::with_capacity(doc.pdf_documents.len());
    for pdf in &doc.pdf_documents {
        let json = serde_json::to_vec(pdf).map_err(|e| PackageError::Encode(e.to_string()))?;
        let (entry, written) = put_content(dir, "assets", "a", &json)?;
        report.assets_written += usize::from(written);
        report.assets_reused += usize::from(!written);
        pdfs.push(entry);
    }
    let mut image_templates = Vec::with_capacity(doc.image_templates.len());
    for tpl in &doc.image_templates {
        let json = serde_json::to_vec(tpl).map_err(|e| PackageError::Encode(e.to_string()))?;
        let (entry, written) = put_content(dir, "assets", "a", &json)?;
        report.assets_written += usize::from(written);
        report.assets_reused += usize::from(!written);
        image_templates.push(entry);
    }

    let manifest = Manifest {
        format: MANIFEST_FORMAT.to_string(),
        version: MANIFEST_VERSION,
        id: doc.id.clone(),
        title: doc.title.clone(),
        cover: doc.cover,
        folder_path: doc.folder_path.clone(),
        favorite: doc.favorite,
        tags: doc.tags.clone(),
        created_at_ms: doc.created_at_ms,
        updated_at_ms: doc.updated_at_ms,
        meta_updated_at_ms: doc.meta_updated_at_ms,
        next_stroke_id: doc.next_stroke_id,
        current_page: doc.active_page_index(),
        flashcards: doc.flashcards.clone(),
        pages: page_entries,
        pdfs,
        image_templates,
    };
    let bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| PackageError::Encode(e.to_string()))?;
    let manifest_path = dir.join(MANIFEST_FILE);
    // The commit point: only now does the new revision become visible.
    atomic_write(&manifest_path, &bytes).map_err(io_err(&manifest_path))?;

    let keep: std::collections::BTreeSet<&str> = manifest
        .pages
        .iter()
        .map(|p| p.entry.file.as_str())
        .chain(manifest.pdfs.iter().map(|e| e.file.as_str()))
        .chain(manifest.image_templates.iter().map(|e| e.file.as_str()))
        .collect();
    report.files_removed = collect_garbage(dir, &keep);
    Ok(report)
}

/// Removes superseded page/asset files and stale temp files. `.corrupt` files are never touched.
fn collect_garbage(dir: &Path, keep: &std::collections::BTreeSet<&str>) -> usize {
    let mut removed = 0;
    for sub in ["pages", "assets"] {
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = format!("{sub}/{name}");
            let is_tmp = name.contains(".tmp.");
            let is_quarantined = name.contains(".corrupt");
            if is_quarantined || (!is_tmp && keep.contains(rel.as_str())) {
                continue;
            }
            if fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

fn safe_relative(rel: &str, sub: &str) -> bool {
    let Some(name) = rel.strip_prefix(&format!("{sub}/")) else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        && !name.starts_with('.')
}

enum ReadFailure {
    Missing,
    Bad(String),
}

/// Reads and verifies one referenced file. Verification failures are reported, not panicked.
fn read_verified(dir: &Path, sub: &str, entry: &FileEntry) -> Result<Vec<u8>, ReadFailure> {
    if !safe_relative(&entry.file, sub) {
        return Err(ReadFailure::Bad("unsafe file name in manifest".to_string()));
    }
    let path = dir.join(&entry.file);
    let meta = match fs::metadata(&path) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Err(ReadFailure::Bad("not a regular file".to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(ReadFailure::Missing),
        Err(e) => return Err(ReadFailure::Bad(e.to_string())),
    };
    if meta.len() > MAX_PACKAGE_FILE_BYTES || meta.len() != entry.bytes {
        return Err(ReadFailure::Bad(format!(
            "size {} does not match manifest ({})",
            meta.len(),
            entry.bytes
        )));
    }
    let bytes = fs::read(&path).map_err(|e| ReadFailure::Bad(e.to_string()))?;
    if sha256_hex(&bytes) != entry.sha256 {
        return Err(ReadFailure::Bad("checksum mismatch".to_string()));
    }
    Ok(bytes)
}

/// Opens a package folder. Corrupt page or asset files are quarantined as `.corrupt`; a lost page
/// becomes a blank stand-in so the rest of the notebook stays usable.
pub fn read_package(dir: &Path) -> Result<OpenedPackage, PackageError> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let raw = match fs::metadata(&manifest_path) {
        Ok(m) if m.len() > MAX_MANIFEST_BYTES => Err("manifest too large".to_string()),
        Ok(_) => fs::read(&manifest_path).map_err(|e| e.to_string()),
        Err(e) => return Err(io_err(&manifest_path)(e)),
    };
    let manifest: Manifest =
        match raw.and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string())) {
            Ok(m) => m,
            Err(reason) => {
                return Err(PackageError::ManifestCorrupt {
                    quarantined: quarantine(&manifest_path).ok(),
                    reason,
                });
            }
        };
    if manifest.format != MANIFEST_FORMAT {
        return Err(PackageError::ManifestCorrupt {
            quarantined: quarantine(&manifest_path).ok(),
            reason: format!("unexpected format `{}`", manifest.format),
        });
    }
    if manifest.version != MANIFEST_VERSION {
        return Err(PackageError::UnsupportedVersion(manifest.version));
    }
    if manifest.pages.is_empty() || manifest.pages.len() > MAX_PACKAGE_PAGES {
        return Err(PackageError::ManifestCorrupt {
            quarantined: quarantine(&manifest_path).ok(),
            reason: "page count out of range".to_string(),
        });
    }
    if manifest.pdfs.len() + manifest.image_templates.len() > MAX_PACKAGE_ASSETS {
        return Err(PackageError::TooLarge("asset"));
    }

    let mut quarantined = Vec::new();
    let mut quarantine_file = |sub: &str, entry: &FileEntry, reason: String| {
        // Never move anything the manifest names outside its own pages/ or assets/ folder.
        if !safe_relative(&entry.file, sub) {
            return;
        }
        let original = dir.join(&entry.file);
        if let Ok(moved_to) = quarantine(&original) {
            quarantined.push(Quarantined {
                original,
                moved_to,
                reason,
            });
        }
    };

    let mut pages = Vec::with_capacity(manifest.pages.len().min(256));
    let mut blank_pages = Vec::new();
    for (idx, page_entry) in manifest.pages.iter().enumerate() {
        let loaded = read_verified(dir, "pages", &page_entry.entry).and_then(|bytes| {
            serde_json::from_slice::<NotebookPage>(&bytes)
                .map_err(|e| ReadFailure::Bad(format!("invalid page data: {e}")))
        });
        match loaded {
            Ok(page) => pages.push(page),
            Err(failure) => {
                if let ReadFailure::Bad(reason) = failure {
                    quarantine_file("pages", &page_entry.entry, reason);
                }
                blank_pages.push(idx);
                pages.push(NotebookPage::new(
                    page_entry.width,
                    page_entry.height,
                    PaperTemplate::Blank,
                    PaperColor::PureWhite,
                ));
            }
        }
    }

    let mut pdf_documents = Vec::with_capacity(manifest.pdfs.len());
    for entry in &manifest.pdfs {
        match read_verified(dir, "assets", entry).and_then(|bytes| {
            serde_json::from_slice::<EmbeddedPdf>(&bytes)
                .map_err(|e| ReadFailure::Bad(format!("invalid PDF asset: {e}")))
        }) {
            Ok(pdf) => pdf_documents.push(pdf),
            Err(failure) => {
                if let ReadFailure::Bad(reason) = failure {
                    quarantine_file("assets", entry, reason);
                }
                // Keep indices stable; sanitization drops the empty payload and its backdrops.
                pdf_documents.push(EmbeddedPdf::default());
            }
        }
    }
    let mut image_templates = Vec::with_capacity(manifest.image_templates.len());
    for entry in &manifest.image_templates {
        match read_verified(dir, "assets", entry).and_then(|bytes| {
            serde_json::from_slice::<EmbeddedImageTemplate>(&bytes)
                .map_err(|e| ReadFailure::Bad(format!("invalid template asset: {e}")))
        }) {
            Ok(tpl) => image_templates.push(tpl),
            Err(failure) => {
                if let ReadFailure::Bad(reason) = failure {
                    quarantine_file("assets", entry, reason);
                }
                image_templates.push(EmbeddedImageTemplate::default());
            }
        }
    }

    let first = manifest.pages.first();
    let mut doc = Document::new(
        first.map_or(1200, |p| p.width),
        first.map_or(1600, |p| p.height),
    );
    doc.id = manifest.id;
    doc.title = manifest.title;
    doc.cover = manifest.cover;
    doc.folder_path = manifest.folder_path;
    doc.favorite = manifest.favorite;
    doc.tags = manifest.tags;
    doc.created_at_ms = manifest.created_at_ms;
    doc.updated_at_ms = manifest.updated_at_ms;
    doc.meta_updated_at_ms = manifest.meta_updated_at_ms;
    doc.next_stroke_id = manifest.next_stroke_id.max(1);
    doc.flashcards = manifest.flashcards;
    doc.pdf_documents = pdf_documents;
    doc.image_templates = image_templates;
    doc.pages = pages;
    doc.current_page = manifest.current_page;
    doc.sanitize_untrusted();
    doc.load_active_from_pages();
    Ok(OpenedPackage {
        document: doc,
        quarantined,
        blank_pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use drawcraft_doc::{InkStroke, PenSettings, StrokeSample, StudyTapeItem};
    use std::sync::Arc;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let p = std::env::temp_dir().join(format!("drawcraft_pkg_{label}_{nanos}"));
            let _ = fs::remove_dir_all(&p);
            Self(p.join(format!("book.{PACKAGE_DIR_EXTENSION}")))
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap_or(&self.0));
        }
    }

    fn doc_with_pages(n: usize) -> Document {
        let mut doc = Document::new(800, 1000);
        doc.title = "Biology".to_string();
        for i in 0..n {
            if i > 0 {
                doc.add_page(None, None);
            }
            let id = doc.alloc_stroke_id();
            let y = 10.0 + i as f64;
            let pts = vec![
                StrokeSample::from_xy_pressure(5.0, y, 0.5),
                StrokeSample::from_xy_pressure(60.0, y + 20.0, 0.9),
            ];
            doc.strokes.push(Arc::new(InkStroke::new(
                id,
                PenSettings::ball_pen(),
                pts.clone(),
                pts,
            )));
            doc.sync_active_to_pages();
        }
        doc.select_page(0);
        doc
    }

    fn page_files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir.join("pages"))
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn package_round_trips_pages_metadata_study_items_and_current_page() -> Result<(), PackageError>
    {
        let tmp = TempDir::new("roundtrip");
        let mut doc = doc_with_pages(3);
        doc.flashcards
            .push(Flashcard::new(1, "Q", "A", Some(2), 10));
        doc.study_tapes
            .push(StudyTapeItem::new(1, 1.0, 2.0, 30.0, 12.0));
        doc.sync_active_to_pages();
        doc.select_page(2);
        write_package(&tmp.0, &doc)?;

        let opened = read_package(&tmp.0)?;
        assert!(opened.quarantined.is_empty() && opened.blank_pages.is_empty());
        let loaded = opened.document;
        assert_eq!(loaded.title, "Biology");
        assert_eq!(loaded.pages.len(), 3);
        assert_eq!(loaded.active_page_index(), 2);
        assert_eq!(loaded.flashcards, doc.flashcards);
        assert_eq!(loaded.pages[0].study_tapes, doc.pages[0].study_tapes);
        assert_eq!(loaded.pages[1].strokes.len(), 1);
        Ok(())
    }

    #[test]
    fn saving_one_edited_page_rewrites_only_that_page() -> Result<(), PackageError> {
        let tmp = TempDir::new("incremental");
        let mut doc = doc_with_pages(4);
        let first = write_package(&tmp.0, &doc)?;
        assert_eq!((first.pages_written, first.pages_reused), (4, 0));

        doc.select_page(1);
        let id = doc.alloc_stroke_id();
        let pts = vec![StrokeSample::from_xy_pressure(1.0, 1.0, 0.5)];
        doc.strokes.push(Arc::new(InkStroke::new(
            id,
            PenSettings::ball_pen(),
            pts.clone(),
            pts,
        )));
        doc.sync_active_to_pages();
        let second = write_package(&tmp.0, &doc)?;
        assert_eq!((second.pages_written, second.pages_reused), (1, 3));
        assert_eq!(
            second.files_removed, 1,
            "superseded page file is garbage-collected"
        );
        assert_eq!(page_files(&tmp.0).len(), 4);

        let unchanged = write_package(&tmp.0, &doc)?;
        assert_eq!((unchanged.pages_written, unchanged.pages_reused), (0, 4));
        Ok(())
    }

    #[test]
    fn a_crash_before_the_manifest_commit_keeps_the_previous_save_intact()
    -> Result<(), PackageError> {
        let tmp = TempDir::new("crash");
        let doc = doc_with_pages(2);
        write_package(&tmp.0, &doc)?;
        let before = fs::read(tmp.0.join(MANIFEST_FILE)).map_err(io_err(&tmp.0))?;

        // Simulate a save that wrote a new page file and died before replacing the manifest.
        atomic_write(&tmp.0.join("pages").join("p-0000000000000000.json"), b"{}")
            .map_err(io_err(&tmp.0))?;
        fs::write(tmp.0.join("manifest.json.tmp.0"), b"{ half written").map_err(io_err(&tmp.0))?;

        assert_eq!(
            fs::read(tmp.0.join(MANIFEST_FILE)).map_err(io_err(&tmp.0))?,
            before
        );
        let opened = read_package(&tmp.0)?;
        assert_eq!(opened.document.pages.len(), 2);
        assert!(opened.quarantined.is_empty());
        Ok(())
    }

    #[test]
    fn corrupt_page_is_quarantined_not_overwritten_and_replaced_by_a_blank_page()
    -> Result<(), PackageError> {
        let tmp = TempDir::new("corrupt-page");
        let doc = doc_with_pages(3);
        write_package(&tmp.0, &doc)?;
        let victim = page_files(&tmp.0).remove(1);
        let victim_path = tmp.0.join("pages").join(&victim);
        fs::write(&victim_path, b"{\"garbage\": true}").map_err(io_err(&victim_path))?;

        let opened = read_package(&tmp.0)?;
        assert_eq!(opened.document.pages.len(), 3, "notebook still opens");
        assert_eq!(opened.quarantined.len(), 1);
        assert_eq!(opened.blank_pages.len(), 1);
        let q = &opened.quarantined[0];
        assert!(q.moved_to.to_string_lossy().ends_with(".corrupt"));
        assert!(!victim_path.exists() && q.moved_to.exists());
        assert_eq!(
            fs::read(&q.moved_to).map_err(io_err(&q.moved_to))?,
            b"{\"garbage\": true}",
            "evidence preserved byte for byte"
        );

        // Saving again must not clobber the quarantined evidence.
        write_package(&tmp.0, &opened.document)?;
        assert!(q.moved_to.exists());
        Ok(())
    }

    #[test]
    fn missing_page_file_degrades_to_a_blank_page_without_quarantine() -> Result<(), PackageError> {
        let tmp = TempDir::new("missing");
        write_package(&tmp.0, &doc_with_pages(2))?;
        let victim = tmp.0.join("pages").join(page_files(&tmp.0).remove(0));
        fs::remove_file(&victim).map_err(io_err(&victim))?;
        let opened = read_package(&tmp.0)?;
        assert_eq!(opened.blank_pages.len(), 1);
        assert!(opened.quarantined.is_empty());
        Ok(())
    }

    #[test]
    fn corrupt_manifest_is_quarantined_and_reported() -> Result<(), PackageError> {
        let tmp = TempDir::new("manifest");
        write_package(&tmp.0, &doc_with_pages(1))?;
        fs::write(tmp.0.join(MANIFEST_FILE), b"not json at all").map_err(io_err(&tmp.0))?;
        match read_package(&tmp.0) {
            Err(PackageError::ManifestCorrupt {
                quarantined: Some(p),
                ..
            }) => {
                assert!(p.exists() && !tmp.0.join(MANIFEST_FILE).exists());
            }
            other => panic!("expected quarantined corrupt manifest, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn hostile_manifests_are_refused_without_touching_the_filesystem_outside()
    -> Result<(), PackageError> {
        let tmp = TempDir::new("hostile");
        let doc = doc_with_pages(1);
        write_package(&tmp.0, &doc)?;
        let path = tmp.0.join(MANIFEST_FILE);
        let text = fs::read_to_string(&path).map_err(io_err(&path))?;
        let entry = read_package(&tmp.0)?; // sanity: valid before tampering
        assert!(entry.quarantined.is_empty());
        let victim = page_files(&tmp.0).remove(0);
        let evil = text.replace(&format!("pages/{victim}"), "pages/../../escape.json");
        fs::write(&path, evil).map_err(io_err(&path))?;
        let outside = tmp.0.join("..").join("escape.json");
        fs::write(&outside, b"precious").map_err(io_err(&outside))?;
        let opened = read_package(&tmp.0)?;
        assert!(
            opened.quarantined.is_empty(),
            "nothing outside the package may be moved"
        );
        assert_eq!(fs::read(&outside).map_err(io_err(&outside))?, b"precious");
        assert_eq!(
            opened.blank_pages,
            vec![0],
            "traversal path is not followed"
        );

        fs::write(&path, br#"{"format":"drawcraft-notebook","version":99,"id":"","title":"","cover":"ClassicNavy","folder_path":"","favorite":false,"tags":[],"created_at_ms":0,"updated_at_ms":0,"meta_updated_at_ms":0,"next_stroke_id":1,"current_page":0,"pages":[]}"#)
            .map_err(io_err(&path))?;
        assert!(matches!(
            read_package(&tmp.0),
            Err(PackageError::UnsupportedVersion(99))
        ));
        Ok(())
    }

    #[test]
    fn quarantine_never_overwrites_an_earlier_quarantine() -> io::Result<()> {
        let tmp = TempDir::new("quarantine");
        fs::create_dir_all(&tmp.0)?;
        let f = tmp.0.join("x.json");
        let mut seen = Vec::new();
        for n in 0..3u8 {
            fs::write(&f, [n])?;
            seen.push(quarantine(&f)?);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 3);
        Ok(())
    }
}
