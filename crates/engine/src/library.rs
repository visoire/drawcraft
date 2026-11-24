//! Optional on-disk `.dcraft` library persistence for desktop sessions.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use drawcraft_doc::{Document, NotebookPreset, UserStudioConfig, sanitize_notebook_title};
use drawcraft_format::{load_document_bytes, save_document_bytes};
use uuid::Uuid;

use crate::{MAX_NOTEBOOKS, Session};

pub(crate) const LIBRARY_MARKER_FILE: &str = ".library_initialized";
pub(crate) const STUDIO_PREFS_FILE: &str = "studio_prefs.json";
const MAX_PREFS_FILE_BYTES: u64 = 20 * 1024 * 1024;
const TRASH_DIR: &str = "trash";
const TRASH_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const MAX_TRASH_NOTEBOOKS: usize = 256;
const MAX_TRASH_SCAN_FILES: usize = 4096;

/// A notebook held for recovery after it is removed from the active library.
#[derive(Clone, Debug)]
pub struct TrashedNotebook {
    pub doc: Arc<Document>,
    pub deleted_at_ms: i64,
    pub(crate) path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LibraryState {
    pub(crate) dir: Option<PathBuf>,
    pub(crate) slot_files: Vec<Option<PathBuf>>,
    pub(crate) slot_dirty: Vec<bool>,
    pub(crate) next_seq: u64,
}

impl LibraryState {
    pub(crate) fn sync_len(&mut self, count: usize) {
        self.slot_files.resize_with(count, || None);
        self.slot_dirty.resize(count, false);
    }

    pub(crate) fn mark_dirty(&mut self, index: usize, count: usize) {
        self.sync_len(count);
        if let Some(slot) = self.slot_dirty.get_mut(index) {
            *slot = true;
        }
    }

    pub(crate) fn push_slot(&mut self, path: Option<PathBuf>, dirty: bool, new_count: usize) {
        if self.slot_files.len() + 1 == new_count {
            self.slot_files.push(path);
            self.slot_dirty.push(dirty);
        } else {
            self.sync_len(new_count);
            if let Some(last) = self.slot_files.last_mut() {
                *last = path;
            }
            if let Some(last) = self.slot_dirty.last_mut() {
                *last = dirty;
            }
        }
    }

    pub(crate) fn remove_slot(&mut self, index: usize) {
        if index < self.slot_files.len()
            && let Some(path) = self.slot_files.remove(index)
            && self.dir.is_some()
        {
            let _ = fs::remove_file(path);
        }
        if index < self.slot_dirty.len() {
            self.slot_dirty.remove(index);
        }
    }

    pub(crate) fn has_dirty(&self) -> bool {
        self.slot_dirty.iter().any(|&dirty| dirty)
    }

    pub(crate) fn alloc_file_path(&mut self, dir: &Path) -> PathBuf {
        let secs = drawcraft_sync_protocol::now_unix_ms() / 1000;
        for _ in 0..10_000 {
            self.next_seq = self.next_seq.saturating_add(1);
            let candidate = dir.join(format!("nb_{secs}_{:04}.dcraft", self.next_seq));
            if !candidate.exists() {
                return candidate;
            }
        }
        self.next_seq = self.next_seq.saturating_add(1);
        dir.join(format!("nb_fallback_{}.dcraft", self.next_seq))
    }
}

/// Returns the default portable DrawCraft library directory.
///
/// - Standalone executables (Windows `.exe`, raw Unix binary): `<exe_dir>/library`
/// - Linux AppImage (`APPIMAGE` env var set): `<appimage_dir>/library` (never the read-only mount)
/// - macOS `.app` bundle (`*.app/Contents/MacOS/*`): `~/Library/Application Support/DrawCraft/library`
pub fn default_library_dir() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_library_dir(&exe_path, appimage.as_deref(), home.as_deref())
}

fn resolve_library_dir(
    exe_path: &Path,
    appimage: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(img) = appimage.filter(|p| !p.as_os_str().is_empty())
        && let Some(parent) = img.parent().filter(|p| !p.as_os_str().is_empty())
    {
        return Some(parent.join("library"));
    }
    if is_inside_macos_app_bundle(exe_path)
        && let Some(home_dir) = home.filter(|p| !p.as_os_str().is_empty())
    {
        return Some(
            home_dir
                .join("Library")
                .join("Application Support")
                .join("DrawCraft")
                .join("library"),
        );
    }
    library_dir_for_exe(exe_path)
}

fn is_inside_macos_app_bundle(exe_path: &Path) -> bool {
    let Some(macos_dir) = exe_path.parent() else {
        return false;
    };
    let Some(contents_dir) = macos_dir.parent() else {
        return false;
    };
    let Some(bundle_dir) = contents_dir.parent() else {
        return false;
    };
    macos_dir.file_name().and_then(|n| n.to_str()) == Some("MacOS")
        && contents_dir.file_name().and_then(|n| n.to_str()) == Some("Contents")
        && bundle_dir
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

fn library_dir_for_exe(exe_path: &Path) -> Option<PathBuf> {
    let exe_dir = exe_path.parent()?;
    if exe_dir.as_os_str().is_empty() {
        return None;
    }
    Some(exe_dir.join("library"))
}

const STARTER_DAILY_NOTEBOOK_BYTES: &[u8] =
    include_bytes!("../assets/starter_daily_notebook.dcraft");

pub(crate) fn starter_documents() -> Vec<Document> {
    let now = unix_now_ms();
    let mut daily = load_document_bytes(STARTER_DAILY_NOTEBOOK_BYTES)
        .unwrap_or_else(|_| NotebookPreset::EverydayLined.create_document());
    daily.paper_color = drawcraft_doc::PaperColor::PureWhite;
    daily.sync_active_to_pages();
    for page in &mut daily.pages {
        page.paper_color = drawcraft_doc::PaperColor::PureWhite;
    }
    daily.load_active_from_pages();

    let mut docs = vec![daily];
    let presets = [
        NotebookPreset::LectureCornell,
        NotebookPreset::EngineeringGrid,
        NotebookPreset::SketchBlank,
    ];
    for preset in presets {
        docs.push(preset.create_document());
    }
    for doc in &mut docs {
        doc.id = Uuid::new_v4().to_string();
        doc.created_at_ms = now;
        doc.updated_at_ms = now;
        doc.meta_updated_at_ms = now;
    }
    docs
}

fn system_time_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn is_notebook_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("dcraft")
                    || ext.eq_ignore_ascii_case("scraft")
                    || ext.eq_ignore_ascii_case("pcraft")
                    || ext.eq_ignore_ascii_case("ncraft")
            })
}

/// Counts non-empty `.dcraft` / `.ncraft` files in `dir` without creating folders or parsing file contents.
pub(crate) fn count_ncraft_files_in_dir(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if is_notebook_file(&path) && entry.metadata().is_ok_and(|meta| meta.len() > 0) {
            count = count.saturating_add(1);
            if count >= MAX_NOTEBOOKS {
                break;
            }
        }
    }
    count
}

struct LoadedEntry {
    modified: SystemTime,
    path: PathBuf,
    doc: Document,
}

fn read_library_entries(dir: &Path) -> Vec<LoadedEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut loaded = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_notebook_file(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(raw) = fs::read(&path) else {
            continue;
        };
        let Ok(mut doc) = load_document_bytes(&raw) else {
            continue;
        };
        doc.title = sanitize_notebook_title(&doc.title);
        doc.sync_active_to_pages();
        doc.load_active_from_pages();
        let modified = meta.modified().unwrap_or(UNIX_EPOCH);
        let modified_ms = system_time_ms(modified);
        let created_ms = meta.created().map(system_time_ms).unwrap_or(modified_ms);
        if doc.updated_at_ms == 0 {
            doc.updated_at_ms = modified_ms.max(doc.created_at_ms);
        }
        if doc.created_at_ms == 0 {
            doc.created_at_ms = created_ms.min(if doc.updated_at_ms > 0 {
                doc.updated_at_ms
            } else {
                created_ms
            });
        }
        loaded.push(LoadedEntry {
            modified,
            path,
            doc,
        });
    }
    loaded.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    loaded.truncate(MAX_NOTEBOOKS);
    loaded
}

fn write_document_file(path: &Path, doc: &Document) -> Result<(), String> {
    let mut snapshot = doc.clone();
    snapshot.sync_active_to_pages();
    let bytes = save_document_bytes(&snapshot)
        .map_err(|err| format!("failed to serialize notebook: {err}"))?;
    drawcraft_sync::write_bytes_atomic(path, &bytes)
        .map_err(|err| format!("failed to write `{}`: {err}", path.display()))
}

fn parse_trash_filename(path: &Path) -> Option<i64> {
    let stem = path.file_stem()?.to_str()?.strip_prefix("trash_")?;
    let mut parts = stem.split('_');
    let deleted_at_ms = parts.next()?.parse::<i64>().ok()?;
    let _sequence = parts.next()?.parse::<u64>().ok()?;
    parts.next().is_none().then_some(deleted_at_ms)
}

fn read_trash_entries(dir: &Path) -> Vec<TrashedNotebook> {
    let trash_dir = dir.join(TRASH_DIR);
    let Ok(entries) = fs::read_dir(&trash_dir) else {
        return Vec::new();
    };
    let now_ms = unix_now_ms();
    let mut paths: Vec<(i64, PathBuf)> = entries
        .flatten()
        .take(MAX_TRASH_SCAN_FILES)
        .filter_map(|entry| {
            let path = entry.path();
            if !is_notebook_file(&path) {
                return None;
            }
            let deleted_at_ms = parse_trash_filename(&path)?;
            Some((deleted_at_ms.min(now_ms), path))
        })
        .collect();
    paths.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut loaded = Vec::new();
    for (deleted_at_ms, path) in paths {
        if now_ms.saturating_sub(deleted_at_ms) >= TRASH_RETENTION_MS {
            let _ = fs::remove_file(path);
            continue;
        }
        if loaded.len() >= MAX_TRASH_NOTEBOOKS {
            continue;
        }
        let Ok(raw) = fs::read(&path) else {
            continue;
        };
        let Ok(mut doc) = load_document_bytes(&raw) else {
            continue;
        };
        doc.title = sanitize_notebook_title(&doc.title);
        doc.sync_active_to_pages();
        doc.load_active_from_pages();
        loaded.push(TrashedNotebook {
            doc: Arc::new(doc),
            deleted_at_ms,
            path: Some(path),
        });
    }
    loaded
}

fn unix_now_ms() -> i64 {
    drawcraft_sync_protocol::now_unix_ms()
}

fn load_prefs_file(dir: &Path) -> UserStudioConfig {
    let path = dir.join(STUDIO_PREFS_FILE);
    let Ok(meta) = fs::metadata(&path) else {
        return UserStudioConfig::default();
    };
    if meta.len() > MAX_PREFS_FILE_BYTES {
        return UserStudioConfig::default();
    }
    let Ok(raw) = fs::read_to_string(&path) else {
        return UserStudioConfig::default();
    };
    serde_json::from_str::<UserStudioConfig>(&raw)
        .map(|cfg| cfg.sanitized())
        .unwrap_or_default()
}

fn write_prefs_file(dir: &Path, prefs: &UserStudioConfig) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| {
        format!(
            "failed to prepare library folder `{}`: {err}",
            dir.display()
        )
    })?;
    let clean = prefs.sanitized();
    let json = serde_json::to_string_pretty(&clean)
        .map_err(|err| format!("failed to serialize studio preferences: {err}"))?;
    let path = dir.join(STUDIO_PREFS_FILE);
    drawcraft_sync::write_bytes_atomic(&path, json.as_bytes())
        .map_err(|err| format!("failed to write `{}`: {err}", path.display()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LibraryInitMode {
    SeedStartersIfFresh,
    BlankPlaceholderOnly,
}

impl Session {
    /// Opens (or initializes) a persistent `.dcraft` notebook library in `dir`.
    pub fn open_library(dir: PathBuf) -> Self {
        Self::open_library_mode(dir, LibraryInitMode::SeedStartersIfFresh).0
    }

    pub(crate) fn open_library_mode(dir: PathBuf, mode: LibraryInitMode) -> (Self, bool) {
        if fs::create_dir_all(&dir).is_err() {
            return (Self::with_starter_library(None), false);
        }

        let prefs = load_prefs_file(&dir);
        let marker = dir.join(LIBRARY_MARKER_FILE);
        let marker_exists = marker.exists();
        let loaded = read_library_entries(&dir);

        if loaded.is_empty() {
            if mode == LibraryInitMode::BlankPlaceholderOnly && !marker_exists {
                let mut session = Self {
                    prefs,
                    ..Default::default()
                };
                session.apply_pen_size_preferences();
                session.library = LibraryState {
                    dir: Some(dir.clone()),
                    slot_files: vec![None],
                    slot_dirty: vec![false],
                    next_seq: 0,
                };
                session.trashed_notebooks = read_trash_entries(&dir);
                return (session, true);
            }
            let docs = if marker_exists {
                Vec::new()
            } else {
                starter_documents()
            };
            let mut session = Self {
                prefs,
                ..Default::default()
            };
            session.apply_pen_size_preferences();
            session.replace_notebooks_for_library(docs, Some(dir));
            if let Some(ref library_dir) = session.library.dir {
                session.trashed_notebooks = read_trash_entries(library_dir);
            }
            let _ = session.save_library();
            let _ = fs::write(&marker, b"1");
            return (session, false);
        }

        if !marker_exists {
            let _ = fs::write(&marker, b"1");
        }

        let mut docs = Vec::with_capacity(loaded.len());
        let mut files = Vec::with_capacity(loaded.len());
        for entry in loaded {
            docs.push(Arc::new(entry.doc));
            files.push(Some(entry.path));
        }

        let mut session = Self::new((*docs[0]).clone());
        let count = docs.len();
        session.prefs = prefs;
        session.apply_pen_size_preferences();
        session.notebooks = docs;
        session.active_notebook = 0;
        session.doc = Arc::clone(&session.notebooks[0]);
        session.library = LibraryState {
            dir: Some(dir),
            slot_files: files,
            slot_dirty: vec![false; count],
            next_seq: count as u64,
        };
        if let Some(ref library_dir) = session.library.dir {
            session.trashed_notebooks = read_trash_entries(library_dir);
        }
        (session, false)
    }

    fn with_starter_library(dir: Option<PathBuf>) -> Self {
        let mut session = Self::default();
        let docs = starter_documents();
        session.replace_notebooks_for_library(docs, dir);
        session
    }

    pub(crate) fn replace_notebooks_for_library(
        &mut self,
        docs: Vec<Document>,
        dir: Option<PathBuf>,
    ) {
        let arc_docs: Vec<Arc<Document>> = docs
            .into_iter()
            .take(MAX_NOTEBOOKS)
            .map(|mut d| {
                d.sync_active_to_pages();
                Arc::new(d)
            })
            .collect();
        let count = arc_docs.len();
        let mut blank = Document::default();
        blank.sync_active_to_pages();
        self.doc = match arc_docs.first() {
            Some(doc) => Arc::clone(doc),
            None => Arc::new(blank),
        };
        let mut lib = LibraryState {
            dir: dir.clone(),
            slot_files: vec![None; count],
            slot_dirty: vec![dir.is_some(); count],
            next_seq: 0,
        };
        if let Some(ref lib_dir) = dir {
            for idx in 0..count {
                let candidate = lib_dir.join(format!("nb_{:04}.dcraft", idx + 1));
                let path = if candidate.exists() {
                    lib.alloc_file_path(lib_dir)
                } else {
                    candidate
                };
                lib.slot_files[idx] = Some(path);
            }
            lib.next_seq = count as u64;
        }
        self.notebooks = arc_docs;
        self.active_notebook = 0;
        self.library = lib;
    }

    /// Returns the active library directory if on-disk persistence is enabled.
    pub fn library_dir(&self) -> Option<&Path> {
        self.library.dir.as_deref()
    }

    /// Returns `true` if any notebook has unsaved changes relative to the library directory.
    pub fn has_dirty_notebooks(&self) -> bool {
        self.library.dir.is_some() && self.library.has_dirty()
    }

    /// Saves custom palettes and user pen presets to `studio_prefs.json` when persistence is enabled.
    pub fn save_prefs(&self) -> Result<(), String> {
        let Some(ref dir) = self.library.dir else {
            return Ok(());
        };
        write_prefs_file(dir, &self.prefs)
    }

    /// Collects `(slot_index, path, doc_snapshot)` for all dirty notebooks and clears their
    /// dirty flags so callers can write snapshots off the UI thread.
    pub fn take_dirty_notebook_snapshots(
        &mut self,
    ) -> Result<Vec<(usize, PathBuf, Arc<Document>)>, String> {
        self.sync_active_notebook_slot();
        let Some(dir) = self.library.dir.clone() else {
            return Ok(Vec::new());
        };
        fs::create_dir_all(&dir).map_err(|err| {
            format!(
                "failed to prepare library folder `{}`: {err}",
                dir.display()
            )
        })?;

        let count = self.notebooks.len();
        self.library.sync_len(count);

        let mut out = Vec::new();
        for idx in 0..count {
            let needs_save = self.library.slot_dirty[idx] || self.library.slot_files[idx].is_none();
            if !needs_save {
                continue;
            }
            let path = match self.library.slot_files[idx].clone() {
                Some(existing) => existing,
                None => {
                    let allocated = self.library.alloc_file_path(&dir);
                    self.library.slot_files[idx] = Some(allocated.clone());
                    allocated
                }
            };
            self.library.slot_dirty[idx] = false;
            out.push((idx, path, Arc::clone(&self.notebooks[idx])));
        }
        Ok(out)
    }

    /// Re-marks a notebook slot dirty if an asynchronous save failed for `path`.
    pub fn mark_slot_dirty_if_matching(&mut self, slot_idx: usize, path: &Path) {
        if self
            .library
            .slot_files
            .get(slot_idx)
            .and_then(Option::as_ref)
            .is_some_and(|existing| existing == path)
        {
            self.library.mark_dirty(slot_idx, self.notebooks.len());
        }
    }

    /// Saves all dirty notebooks in `self.notebooks` to `self.library_dir()`.
    pub fn save_library(&mut self) -> Result<usize, String> {
        let snapshots = self.take_dirty_notebook_snapshots()?;
        let mut saved = 0usize;
        for (idx, (_, path, doc)) in snapshots.iter().enumerate() {
            if let Err(err) = write_document_file(path, doc) {
                for (rem_slot, rem_path, _) in &snapshots[idx..] {
                    self.mark_slot_dirty_if_matching(*rem_slot, rem_path);
                }
                return Err(err);
            }
            saved = saved.saturating_add(1);
        }
        Ok(saved)
    }

    /// Moves an active notebook into the 30-day recovery area without discarding its pages.
    pub(crate) fn move_notebook_to_trash(&mut self, index: usize) -> Result<(), String> {
        self.cleanup_expired_trash();
        if index >= self.notebooks.len() {
            return Err("notebook index out of bounds".to_string());
        }
        if self.trashed_notebooks.len() >= MAX_TRASH_NOTEBOOKS {
            return Err(
                "Trash is full. Restore an item or wait for expired items to clear before deleting another notebook."
                    .to_string(),
            );
        }

        let Some(doc) = self.notebooks.get(index).cloned() else {
            return Err("notebook index out of bounds".to_string());
        };
        let bytes = save_document_bytes(&doc)
            .map_err(|err| format!("failed to serialize notebook for Trash: {err}"))?;
        let deleted_at_ms = unix_now_ms();
        let mut trash_entry = TrashedNotebook {
            doc: Arc::clone(&doc),
            deleted_at_ms,
            path: None,
        };

        if let Some(dir) = self.library.dir.clone() {
            let trash_dir = dir.join(TRASH_DIR);
            fs::create_dir_all(&trash_dir).map_err(|err| {
                format!(
                    "failed to prepare Trash folder `{}`: {err}",
                    trash_dir.display()
                )
            })?;
            let mut path = None;
            for _ in 0..10_000_u64 {
                self.library.next_seq = self.library.next_seq.saturating_add(1);
                let candidate = trash_dir.join(format!(
                    "trash_{}_{}.dcraft",
                    deleted_at_ms, self.library.next_seq
                ));
                if !candidate.exists() {
                    path = Some(candidate);
                    break;
                }
            }
            let Some(path) = path else {
                return Err("could not allocate a file name in Trash".to_string());
            };
            drawcraft_sync::write_bytes_atomic(&path, &bytes)
                .map_err(|err| format!("failed to save notebook in Trash: {err}"))?;

            if let Some(source) = self.library.slot_files.get(index).and_then(Option::as_ref)
                && source.exists()
                && let Err(err) = fs::remove_file(source)
            {
                let _ = fs::remove_file(&path);
                return Err(format!(
                    "failed to remove the notebook from the library: {err}"
                ));
            }
            trash_entry.path = Some(path);
        }
        self.trashed_notebooks.push(trash_entry);
        Ok(())
    }

    /// Stores a trashed notebook into the active Trash list and persists it on disk if enabled.
    pub(crate) fn add_trash_entry_persisted(
        &mut self,
        mut doc: Document,
        deleted_at_ms: i64,
    ) -> Result<(), String> {
        if self.trashed_notebooks.len() >= MAX_TRASH_NOTEBOOKS {
            return Err(
                "Trash is full. Restore an item or wait for expired items to clear before importing more trash."
                    .to_string(),
            );
        }
        doc.sync_active_to_pages();
        let doc_arc = Arc::new(doc);
        let mut trash_entry = TrashedNotebook {
            doc: Arc::clone(&doc_arc),
            deleted_at_ms,
            path: None,
        };
        if let Some(dir) = self.library.dir.clone() {
            let trash_dir = dir.join(TRASH_DIR);
            fs::create_dir_all(&trash_dir).map_err(|err| {
                format!(
                    "failed to prepare Trash folder `{}`: {err}",
                    trash_dir.display()
                )
            })?;
            let mut path = None;
            for _ in 0..10_000_u64 {
                self.library.next_seq = self.library.next_seq.saturating_add(1);
                let candidate = trash_dir.join(format!(
                    "trash_{}_{}.dcraft",
                    deleted_at_ms, self.library.next_seq
                ));
                if !candidate.exists() {
                    path = Some(candidate);
                    break;
                }
            }
            let Some(path) = path else {
                return Err("could not allocate a file name in Trash".to_string());
            };
            write_document_snapshot(&path, &doc_arc)?;
            trash_entry.path = Some(path);
        }
        self.trashed_notebooks.push(trash_entry);
        Ok(())
    }

    /// Deletes all Trash entries cleanly from memory and disk.
    pub(crate) fn clear_trash_persisted(&mut self) {
        for entry in self.trashed_notebooks.drain(..) {
            if let Some(path) = entry.path.as_ref() {
                let _ = fs::remove_file(path);
            }
        }
        if let Some(dir) = self.library.dir.as_ref() {
            let trash_dir = dir.join(TRASH_DIR);
            if trash_dir.is_dir() {
                let _ = fs::remove_dir_all(trash_dir);
            }
        }
    }

    /// Restores a notebook from Trash and makes it the active notebook.
    pub(crate) fn restore_trashed_notebook(&mut self, index: usize) -> Result<(), String> {
        if self.notebooks.len() >= MAX_NOTEBOOKS {
            return Err(format!(
                "library is limited to {MAX_NOTEBOOKS} open notebooks"
            ));
        }
        if index >= self.trashed_notebooks.len() {
            return Err("trash index out of bounds".to_string());
        }

        let entry = self
            .trashed_notebooks
            .get(index)
            .cloned()
            .ok_or_else(|| "trash item is no longer available".to_string())?;
        if unix_now_ms().saturating_sub(entry.deleted_at_ms) >= TRASH_RETENTION_MS {
            self.cleanup_expired_trash();
            return Err("this notebook has expired from Trash".to_string());
        }
        self.cleanup_expired_trash();
        let Some(index) =
            self.trashed_notebooks
                .iter()
                .position(|item| match (&entry.path, &item.path) {
                    (Some(target), Some(candidate)) => target == candidate,
                    _ => Arc::ptr_eq(&entry.doc, &item.doc),
                })
        else {
            return Err("trash item is no longer available".to_string());
        };
        let Some(entry) = self.trashed_notebooks.get(index).cloned() else {
            return Err("trash item is no longer available".to_string());
        };
        let restored_path = if let Some(dir) = self.library.dir.clone() {
            let path = self.library.alloc_file_path(&dir);
            if let Some(source) = entry.path.as_ref() {
                if source.exists() {
                    fs::rename(source, &path)
                        .map_err(|err| format!("failed to restore notebook from Trash: {err}"))?;
                } else {
                    write_document_file(&path, &entry.doc)?;
                }
            } else {
                write_document_file(&path, &entry.doc)?;
            }
            Some(path)
        } else {
            None
        };

        let mut restored_doc = (*entry.doc).clone();
        if restored_doc.id.is_empty() {
            restored_doc.id = Uuid::new_v4().to_string();
        }
        let restored_id = restored_doc.id.clone();
        let restored_arc = Arc::new(restored_doc);

        self.trashed_notebooks.remove(index);
        self.notebooks.push(Arc::clone(&restored_arc));
        let new_index = self.notebooks.len().saturating_sub(1);
        self.library
            .push_slot(restored_path.clone(), false, self.notebooks.len());
        self.active_notebook = new_index;
        self.doc = Arc::clone(&restored_arc);
        self.history.clear();
        self.sync_restore_notebook_slot(new_index, &restored_id);
        self.bump_revision();
        Ok(())
    }

    /// Removes items whose 30-day recovery period has ended.
    pub(crate) fn cleanup_expired_trash(&mut self) {
        let now_ms = unix_now_ms();
        let mut keep = Vec::with_capacity(self.trashed_notebooks.len());
        for entry in self.trashed_notebooks.drain(..) {
            if now_ms.saturating_sub(entry.deleted_at_ms) >= TRASH_RETENTION_MS {
                if let Some(path) = entry.path.as_ref() {
                    let _ = fs::remove_file(path);
                }
            } else {
                keep.push(entry);
            }
        }
        self.trashed_notebooks = keep;
    }
}

/// Writes a single notebook snapshot to `path` using atomic temp-file replacement.
pub fn write_document_snapshot(path: &Path, doc: &Document) -> Result<(), String> {
    write_document_file(path, doc)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drawcraft_doc::{NotebookCover, PenSettings, StrokeSample};
    use serde_json::json;

    struct TempDirGuard(PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_lib_dir(name: &str) -> TempDirGuard {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("drawcraft_test_{name}_{nanos}"));
        let _ = fs::remove_dir_all(&dir);
        TempDirGuard(dir)
    }

    #[test]
    fn open_library_seeds_starters_and_persists_edits_across_reopen() {
        let guard = temp_lib_dir("persist");
        let dir = guard.0.clone();

        {
            let mut session = Session::open_library(dir.clone());
            assert_eq!(session.notebooks.len(), 4);
            assert!(!session.has_dirty_notebooks());

            // Select second notebook, rename it, draw a stroke, and create a new one.
            assert!(session.switch_notebook(1));
            session
                .execute(
                    "notebook.rename",
                    &json!({ "title": "Organic Chemistry", "cover": NotebookCover::SageGreen }),
                )
                .unwrap();
            session.commit_raw_stroke(
                PenSettings::fountain_pen(),
                vec![
                    StrokeSample::from_xy_pressure(30.0, 40.0, 0.5),
                    StrokeSample::from_xy_pressure(120.0, 80.0, 0.8),
                ],
            );
            assert!(session.has_dirty_notebooks());
            let saved = session.save_library().unwrap();
            assert_eq!(saved, 1);
            assert!(!session.has_dirty_notebooks());

            session
                .execute(
                    "notebook.create",
                    &json!({ "preset": "MidnightSlate", "title": "Night Calculus" }),
                )
                .unwrap();
            assert_eq!(session.notebooks.len(), 5);

            // Delete the first notebook and ensure deletion persists on disk too.
            session
                .execute("notebook.delete", &json!({ "index": 0 }))
                .unwrap();
            assert_eq!(session.notebooks.len(), 4);
        }

        // Reopen from the same directory: deleted starter stays deleted, edits & created book survive.
        let reopened = Session::open_library(dir);
        assert_eq!(reopened.notebooks.len(), 4);
        let titles: Vec<&str> = reopened
            .notebooks
            .iter()
            .map(|d| d.title.as_str())
            .collect();
        assert!(titles.contains(&"Organic Chemistry"));
        assert!(titles.contains(&"Night Calculus"));

        let chem = reopened
            .notebooks
            .iter()
            .find(|d| d.title == "Organic Chemistry")
            .unwrap();
        assert_eq!(chem.cover, NotebookCover::SageGreen);
        assert_eq!(chem.strokes.len(), 1);
    }

    #[test]
    fn custom_palettes_and_pen_presets_persist_across_reopen() {
        let guard = temp_lib_dir("studio_prefs");
        let dir = guard.0.clone();

        {
            let mut session = Session::open_library(dir.clone());
            session
                .execute(
                    "palette.savePreset",
                    &json!({
                        "name": "Studio Custom",
                        "copy_from_active": true,
                        "select": true,
                    }),
                )
                .unwrap();
            session
                .execute(
                    "palette.setColor",
                    &json!({ "target": "ink", "index": 0, "color_hex": "#AA22CC" }),
                )
                .unwrap();
            session
                .execute(
                    "palette.setColor",
                    &json!({ "target": "ink", "color_hex": "#128877", "apply_to_pen": false }),
                )
                .unwrap();
            session
                .execute(
                    "palette.setWildcard",
                    &json!({ "target": "ink", "color_hex": "#EE4499", "apply_to_pen": false }),
                )
                .unwrap();
            session
                .execute(
                    "palette.setWildcard",
                    &json!({ "target": "highlighter", "color_hex": "#55DD99", "apply_to_pen": false }),
                )
                .unwrap();

            let mut custom_pen = PenSettings::brush_pen();
            custom_pen.size = 15.5;
            custom_pen.opacity = 0.82;
            custom_pen.color = drawcraft_color::Color::from_hex("#128877").unwrap();
            let saved = session
                .execute(
                    "preset.save",
                    &json!({
                        "name": "Studio Marker",
                        "description": "Soft translucent teal brush",
                        "settings": custom_pen,
                        "include_color": true,
                        "apply_now": true
                    }),
                )
                .unwrap();
            assert_eq!(saved["count"], 1);

            session
                .execute(
                    "theme.set",
                    &json!({ "theme": "Dark", "auto_contrast_with_theme": true, "ui_scale": 1.25 }),
                )
                .unwrap();
        }

        let mut reopened = Session::open_library(dir.clone());
        assert_eq!(reopened.prefs.theme, drawcraft_doc::UiTheme::Dark);
        assert!(reopened.prefs.auto_contrast_with_theme);
        assert!((reopened.prefs.ui_scale - 1.25).abs() < 1e-6);
        assert_eq!(reopened.prefs.ink_palette[0].to_hex(), "#AA22CC");
        assert_eq!(
            reopened.prefs.ink_palette.last().unwrap().to_hex(),
            "#128877"
        );
        assert_eq!(reopened.prefs.wildcard_ink.to_hex(), "#EE4499");
        assert_eq!(reopened.prefs.wildcard_highlighter.to_hex(), "#55DD99");
        assert_eq!(reopened.prefs.custom_presets.len(), 1);
        assert_eq!(reopened.prefs.custom_presets[0].name, "Studio Marker");

        // Legacy studio_prefs.json without auto_contrast_with_theme, ui_scale, or wildcards defaults cleanly.
        let legacy: UserStudioConfig = serde_json::from_str(r#"{"theme":"Dark"}"#).unwrap();
        assert_eq!(legacy.theme, drawcraft_doc::UiTheme::Dark);
        assert!(!legacy.auto_contrast_with_theme);
        assert!((legacy.ui_scale - 1.0).abs() < 1e-6);
        assert_eq!(
            legacy.wildcard_ink,
            UserStudioConfig::default_wildcard_ink()
        );
        assert_eq!(
            legacy.wildcard_highlighter,
            UserStudioConfig::default_wildcard_highlighter()
        );

        // Partial theme.set toggling auto_contrast_with_theme preserves theme and ui_scale.
        reopened
            .execute("theme.set", &json!({ "auto_contrast_with_theme": false }))
            .unwrap();
        assert_eq!(reopened.prefs.theme, drawcraft_doc::UiTheme::Dark);
        assert!(!reopened.prefs.auto_contrast_with_theme);
        assert!((reopened.prefs.ui_scale - 1.25).abs() < 1e-6);

        // Partial theme.set updating ui_scale clamps cleanly and preserves theme.
        reopened
            .execute("theme.set", &json!({ "ui_scale": 10.0 }))
            .unwrap();
        assert_eq!(reopened.prefs.theme, drawcraft_doc::UiTheme::Dark);
        assert!((reopened.prefs.ui_scale - UserStudioConfig::MAX_UI_SCALE).abs() < 1e-6);

        let id = reopened.prefs.custom_presets[0].id.clone();
        reopened
            .execute("preset.applyCustom", &json!({ "id": id }))
            .unwrap();
        assert!((reopened.pen(drawcraft_doc::PenKind::BrushPen).size - 15.5).abs() < 1e-4);
        assert_eq!(
            reopened
                .pen(drawcraft_doc::PenKind::BrushPen)
                .color
                .to_hex(),
            "#128877"
        );
    }

    #[test]
    fn palette_presets_lifecycle_protection_and_persistence() {
        let guard = temp_lib_dir("palette_presets");
        let dir = guard.0.clone();

        let created_id = {
            let mut session = Session::open_library(dir.clone());
            assert_eq!(session.prefs.palette_presets.len(), 3);
            assert_eq!(
                session.prefs.active_palette_id,
                drawcraft_doc::BUILTIN_PALETTE_CLASSIC_ID
            );

            // Built-in defaults are strictly read-only: cannot be deleted, renamed, or mutated.
            assert!(
                session
                    .execute(
                        "palette.deletePreset",
                        &json!({ "id": drawcraft_doc::BUILTIN_PALETTE_PASTEL_ID }),
                    )
                    .is_err()
            );
            assert!(
                session
                    .execute(
                        "palette.savePreset",
                        &json!({
                            "id": drawcraft_doc::BUILTIN_PALETTE_EARTH_ID,
                            "name": "Hijacked Name",
                        }),
                    )
                    .is_err()
            );
            assert!(
                session
                    .execute(
                        "palette.savePreset",
                        &json!({
                            "id": drawcraft_doc::BUILTIN_PALETTE_CLASSIC_ID,
                            "name": "Classic",
                            "ink_colors": [drawcraft_color::Color::BLACK],
                        }),
                    )
                    .is_err()
            );
            assert!(
                session
                    .execute(
                        "palette.setColor",
                        &json!({
                            "preset_id": drawcraft_doc::BUILTIN_PALETTE_CLASSIC_ID,
                            "target": "ink",
                            "index": 0,
                            "color_hex": "#FF0000",
                        }),
                    )
                    .is_err()
            );
            assert!(
                session
                    .execute(
                        "palette.removeColor",
                        &json!({
                            "preset_id": drawcraft_doc::BUILTIN_PALETTE_CLASSIC_ID,
                            "target": "ink",
                            "index": 0,
                        }),
                    )
                    .is_err()
            );

            // Duplicating an inactive built-in preset by ID copies its factory swatches.
            let dup = session
                .execute(
                    "palette.savePreset",
                    &json!({
                        "name": "Earth Copy",
                        "copy_from_id": drawcraft_doc::BUILTIN_PALETTE_EARTH_ID,
                        "select": false,
                    }),
                )
                .unwrap();
            let dup_id = dup["id"].as_str().unwrap().to_string();
            let dup_pal = session.prefs.palette_by_id(&dup_id).unwrap();
            assert!(!dup_pal.built_in);
            assert_eq!(
                dup_pal.ink_colors,
                drawcraft_doc::ColorPalettePreset::builtin_earth().ink_colors
            );
            session
                .execute("palette.deletePreset", &json!({ "id": dup_id }))
                .unwrap();

            // Switch to Pastel built-in preset and verify quick swatches update immediately.
            session
                .execute(
                    "palette.selectPreset",
                    &json!({ "id": drawcraft_doc::BUILTIN_PALETTE_PASTEL_ID }),
                )
                .unwrap();
            assert_eq!(
                session.prefs.ink_palette[0],
                drawcraft_color::PASTEL_INK_PALETTE[0].color
            );

            // Create a new user palette preset, add/edit a swatch, and rename it.
            let res = session
                .execute(
                    "palette.savePreset",
                    &json!({
                        "name": "Study Group",
                        "seed_color_hex": "#2255AA",
                    }),
                )
                .unwrap();
            let id = res["id"].as_str().unwrap().to_string();
            assert_eq!(session.prefs.palette_presets.len(), 4);
            assert_eq!(session.prefs.ink_palette.len(), 1);
            assert_eq!(session.prefs.ink_palette[0].to_hex(), "#2255AA");

            session
                .execute(
                    "palette.setColor",
                    &json!({ "target": "ink", "color_hex": "#44AA66", "apply_to_pen": false }),
                )
                .unwrap();
            session
                .execute(
                    "palette.savePreset",
                    &json!({ "id": id, "name": "Biology Lab" }),
                )
                .unwrap();
            id
        };

        let mut reopened = Session::open_library(dir);
        assert_eq!(reopened.prefs.palette_presets.len(), 4);
        assert_eq!(reopened.prefs.active_palette_id, created_id);
        let active = reopened.prefs.active_palette().unwrap();
        assert_eq!(active.name, "Biology Lab");
        assert!(!active.built_in);
        assert_eq!(reopened.prefs.ink_palette.len(), 2);

        // Deleting custom preset switches cleanly back without losing the 3 built-ins.
        reopened
            .execute("palette.deletePreset", &json!({ "id": created_id }))
            .unwrap();
        assert_eq!(reopened.prefs.palette_presets.len(), 3);
        assert!(reopened.prefs.palette_presets.iter().all(|p| p.built_in));
    }

    #[test]
    fn dirty_snapshots_and_matching_error_rollback_work_cleanly() {
        let dir = temp_lib_dir("dirty_snapshots");
        let mut session = Session::open_library(dir.0.clone());
        session.save_library().unwrap();
        assert!(session.take_dirty_notebook_snapshots().unwrap().is_empty());

        session
            .execute("page.setTemplate", &json!({ "template": "Dotted" }))
            .unwrap();

        let snaps = session.take_dirty_notebook_snapshots().unwrap();
        assert_eq!(snaps.len(), 1);
        let (slot_idx, ref path, ref doc) = snaps[0];
        assert_eq!(doc.paper_template, drawcraft_doc::PaperTemplate::Dotted);
        assert!(session.take_dirty_notebook_snapshots().unwrap().is_empty());

        write_document_snapshot(path, doc).unwrap();

        session.mark_slot_dirty_if_matching(slot_idx, Path::new("wrong-slot-path.dcraft"));
        assert!(session.take_dirty_notebook_snapshots().unwrap().is_empty());

        session.mark_slot_dirty_if_matching(slot_idx, path);
        assert_eq!(session.take_dirty_notebook_snapshots().unwrap().len(), 1);

        let reopened = Session::open_library(dir.0.clone());
        assert_eq!(
            reopened.doc.paper_template,
            drawcraft_doc::PaperTemplate::Dotted
        );
    }

    #[test]
    fn trash_cannot_be_deleted_manually_and_expires_automatically() {
        let dir = temp_lib_dir("trash_auto_expire");
        let mut session = Session::open_library(dir.0.clone());

        session
            .execute("notebook.delete", &json!({ "index": 0 }))
            .unwrap();
        assert_eq!(session.trashed_notebooks.len(), 1);

        // Manual deletion from Trash is disabled.
        assert!(
            session
                .execute("notebook.trash.delete", &json!({ "index": 0 }))
                .is_err()
        );
        assert_eq!(session.trashed_notebooks.len(), 1);

        // Non-expired items remain after cleanup and can be restored.
        let kept = session
            .execute("notebook.trash.cleanupExpired", &serde_json::Value::Null)
            .unwrap();
        assert_eq!(kept["removed"], 0);
        assert_eq!(kept["trashCount"], 1);

        // Expired items are removed automatically by the system cleanup.
        session.trashed_notebooks[0].deleted_at_ms =
            unix_now_ms().saturating_sub(TRASH_RETENTION_MS + 1_000);
        let expired = session
            .execute("notebook.trash.cleanupExpired", &serde_json::Value::Null)
            .unwrap();
        assert_eq!(expired["removed"], 1);
        assert_eq!(expired["trashCount"], 0);
        assert!(session.trashed_notebooks.is_empty());
    }

    #[test]
    fn notebook_create_and_rename_persist_custom_cover_color() {
        let dir = temp_lib_dir("custom_cover");
        {
            let mut session = Session::open_library(dir.0.clone());
            session
                .execute(
                    "notebook.create",
                    &json!({
                        "title": "Design Log",
                        "cover": NotebookCover::custom_rgb(0x2B, 0x7A, 0x78),
                    }),
                )
                .unwrap();
            assert_eq!(
                session.doc.cover,
                NotebookCover::custom_rgb(0x2B, 0x7A, 0x78)
            );

            session
                .execute("notebook.rename", &json!({ "cover_hex": "#C2410C" }))
                .unwrap();
            assert_eq!(
                session.doc.cover,
                NotebookCover::custom_rgb(0xC2, 0x41, 0x0C)
            );
            assert!(
                session
                    .execute("notebook.rename", &json!({ "cover_hex": "not-hex" }))
                    .is_err()
            );
            session.save_library().unwrap();
        }

        let reopened = Session::open_library(dir.0.clone());
        let found = reopened
            .notebooks
            .iter()
            .find(|d| d.title == "Design Log")
            .unwrap();
        assert_eq!(found.cover, NotebookCover::custom_rgb(0xC2, 0x41, 0x0C));
    }

    #[test]
    fn folders_favorites_tags_view_prefs_and_custom_templates_persist() {
        let dir = temp_lib_dir("library_org_persist");
        {
            let mut s = Session::open_library(dir.0.clone());
            s.execute(
                "library.folder.create",
                &json!({ "path": "University/Semester1/Calculus" }),
            )
            .unwrap();
            assert_eq!(
                s.prefs.library_folders,
                vec![
                    "University",
                    "University/Semester1",
                    "University/Semester1/Calculus"
                ]
            );
            s.execute(
                "notebook.setMetadata",
                &json!({
                    "index": 0,
                    "folder_path": "University/Semester1/Calculus",
                    "favorite": true,
                    "tags": ["midterm", "#math"],
                }),
            )
            .unwrap();
            s.execute(
                "library.folder.rename",
                &json!({ "old_path": "University", "new_path": "College" }),
            )
            .unwrap();
            assert_eq!(s.notebooks[0].folder_path, "College/Semester1/Calculus");

            s.execute(
                "library.setViewPrefs",
                &json!({
                    "view_mode": "list",
                    "sort_by": "name",
                    "sort_ascending": true,
                }),
            )
            .unwrap();

            let png = drawcraft_raster::png_encode(4, 4, &[250, 240, 220, 255].repeat(16)).unwrap();
            let tpl = s
                .execute(
                    "template.saveFromImage",
                    &json!({ "name": "Cream Card", "bytes": png }),
                )
                .unwrap();
            let tpl_id = tpl["id"].as_str().unwrap().to_string();
            s.execute("page.add", &json!({ "custom_template_id": tpl_id }))
                .unwrap();
            assert!(s.doc.custom_image_backdrop.is_some());

            s.execute(
                "library.folder.delete",
                &json!({ "path": "College/Semester1", "move_notebooks_to": "College" }),
            )
            .unwrap();
            assert_eq!(s.notebooks[0].folder_path, "College");
            s.save_library().unwrap();
        }

        let reopened = Session::open_library(dir.0.clone());
        assert_eq!(
            reopened.prefs.library_view_mode,
            drawcraft_doc::LibraryViewMode::List
        );
        assert_eq!(
            reopened.prefs.library_sort_by,
            drawcraft_doc::LibrarySortBy::Name
        );
        assert!(reopened.prefs.library_sort_ascending);
        assert_eq!(reopened.prefs.custom_templates.len(), 1);
        assert_eq!(reopened.prefs.library_folders, vec!["College"]);
        let moved = reopened
            .notebooks
            .iter()
            .find(|d| d.favorite)
            .expect("favorited notebook survives");
        assert_eq!(moved.folder_path, "College");
        assert_eq!(moved.tags, vec!["midterm", "math"]);
        assert!(moved.updated_at_ms > 0);
        assert!(
            moved
                .pages
                .iter()
                .any(|p| p.custom_image_backdrop.is_some())
        );
    }

    #[test]
    fn large_embedded_pdf_notebook_persists_reloads_and_moves_to_trash() {
        let dir = temp_lib_dir("large_pdf_library");
        let mut large_pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>\nendobj\n%".to_vec();
        large_pdf.resize(34 * 1024 * 1024, b'a');
        large_pdf.extend_from_slice(b"\n%%EOF\n");

        {
            let mut s = Session::open_library(dir.0.clone());
            let mut doc = Document::new(600, 800);
            doc.title = "Heavy PDF".to_string();
            doc.pdf_documents.push(drawcraft_doc::EmbeddedPdf {
                name: "Heavy.pdf".to_string(),
                bytes: Arc::new(large_pdf.clone()),
            });
            doc.pdf_backdrop = Some(drawcraft_doc::PdfPageBackdrop {
                pdf_index: 0,
                page_index: 0,
            });
            doc.sync_active_to_pages();
            s.notebooks.push(Arc::new(doc));
            s.library.push_slot(None, true, s.notebooks.len());
            s.save_library().unwrap();
        }

        let mut reopened = Session::open_library(dir.0.clone());
        let idx = reopened
            .notebooks
            .iter()
            .position(|d| d.title == "Heavy PDF")
            .expect("large PDF notebook should reload from library");
        assert_eq!(
            reopened.notebooks[idx].pdf_documents[0].bytes.len(),
            large_pdf.len()
        );

        reopened
            .execute("notebook.delete", &json!({ "index": idx }))
            .unwrap();
        assert_eq!(reopened.trashed_notebooks.len(), 1);
        reopened
            .execute("notebook.restore", &json!({ "index": 0 }))
            .unwrap();
        assert!(reopened.notebooks.iter().any(|d| d.title == "Heavy PDF"));
    }

    #[test]
    fn folder_colors_create_set_rename_delete_and_persist() {
        let dir = temp_lib_dir("folder_colors_persist");
        let blue = drawcraft_color::Color::from_hex("#2563EB").unwrap();
        let rose = drawcraft_color::Color::from_hex("#DB2777").unwrap();
        {
            let mut s = Session::open_library(dir.0.clone());
            s.execute(
                "library.folder.create",
                &json!({ "path": "School/Math", "color_hex": "#2563EB" }),
            )
            .unwrap();
            assert_eq!(s.prefs.folder_color("School/Math"), Some(blue));

            s.execute(
                "library.folder.setColor",
                &json!({ "path": "School", "color_hex": "#DB2777" }),
            )
            .unwrap();
            assert_eq!(s.prefs.folder_color("school"), Some(rose));

            s.execute(
                "library.folder.rename",
                &json!({ "old_path": "School", "new_path": "College" }),
            )
            .unwrap();
            assert_eq!(s.prefs.folder_color("School"), None);
            assert_eq!(s.prefs.folder_color("College"), Some(rose));
            assert_eq!(s.prefs.folder_color("College/Math"), Some(blue));

            s.execute("library.folder.delete", &json!({ "path": "College/Math" }))
                .unwrap();
            assert_eq!(s.prefs.folder_color("College/Math"), None);
            assert_eq!(s.prefs.folder_color("College"), Some(rose));

            // Clearing color works without deleting the folder itself.
            s.execute(
                "library.folder.create",
                &json!({ "path": "Work", "color_hex": "#16A34A" }),
            )
            .unwrap();
            s.execute(
                "library.folder.setColor",
                &json!({ "path": "Work", "clear": true }),
            )
            .unwrap();
            assert_eq!(s.prefs.folder_color("Work"), None);
            assert!(s.prefs.library_folders.iter().any(|f| f == "Work"));
        }

        let reopened = Session::open_library(dir.0.clone());
        assert_eq!(reopened.prefs.folder_color("College"), Some(rose));
        assert_eq!(reopened.prefs.folder_color("Work"), None);
    }

    #[test]
    fn tag_colors_set_clear_and_persist() {
        let dir = temp_lib_dir("tag_colors_persist");
        let blue = drawcraft_color::Color::from_hex("#2563EB").unwrap();
        let rose = drawcraft_color::Color::from_hex("#DB2777").unwrap();
        {
            let mut s = Session::open_library(dir.0.clone());
            s.execute(
                "library.tag.setColor",
                &json!({ "tag": "#Exam", "color_hex": "#2563EB" }),
            )
            .unwrap();
            assert_eq!(s.prefs.tag_color("exam"), Some(blue));

            s.execute(
                "library.tag.setColor",
                &json!({ "tag": "exam", "color_hex": "#DB2777" }),
            )
            .unwrap();
            assert_eq!(s.prefs.tag_color("EXAM"), Some(rose));

            s.execute(
                "library.tag.setColor",
                &json!({ "tag": "temp", "color_hex": "#16A34A" }),
            )
            .unwrap();
            s.execute(
                "library.tag.setColor",
                &json!({ "tag": "TEMP", "clear": true }),
            )
            .unwrap();
            assert_eq!(s.prefs.tag_color("temp"), None);

            assert!(
                s.execute(
                    "library.tag.setColor",
                    &json!({ "tag": "  ### ", "color_hex": "#2563EB" }),
                )
                .is_err()
            );
        }

        let reopened = Session::open_library(dir.0.clone());
        assert_eq!(reopened.prefs.tag_color("exam"), Some(rose));
        assert_eq!(reopened.prefs.tag_color("temp"), None);
    }

    #[test]
    fn default_library_dir_resolves_library_beside_executable() {
        let exe_dir = std::env::temp_dir().join("drawcraft_portable_app");
        let exe_path = exe_dir.join("drawcraft.exe");
        assert_eq!(
            library_dir_for_exe(&exe_path),
            Some(exe_dir.join("library"))
        );
        assert_eq!(library_dir_for_exe(Path::new("drawcraft.exe")), None);
        assert_eq!(library_dir_for_exe(Path::new("")), None);

        let current_exe_parent = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        assert_eq!(
            default_library_dir(),
            current_exe_parent.map(|dir| dir.join("library"))
        );

        let appimage_exe = Path::new("/tmp/.mount_PaperC123/usr/bin/drawcraft");
        let appimage_file = Path::new("/home/alex/Apps/drawcraft-linux-x86_64.AppImage");
        assert_eq!(
            resolve_library_dir(
                appimage_exe,
                Some(appimage_file),
                Some(Path::new("/home/alex"))
            ),
            Some(PathBuf::from("/home/alex/Apps/library"))
        );

        let mac_bundle_exe = Path::new("/Applications/DrawCraft.app/Contents/MacOS/drawcraft");
        assert_eq!(
            resolve_library_dir(mac_bundle_exe, None, Some(Path::new("/Users/alex"))),
            Some(PathBuf::from(
                "/Users/alex/Library/Application Support/DrawCraft/library"
            ))
        );
    }

    #[test]
    fn open_desktop_library_recovers_from_corrupted_files_and_stale_temps() {
        let guard = temp_lib_dir("corrupt_recovery");
        let dir = guard.0.clone();

        let profile_id = {
            let mut session = Session::open_desktop_library(dir.clone());
            session
                .execute(
                    "profile.completeFirstAccess",
                    &json!({ "nickname": "Alex" }),
                )
                .unwrap();
            session
                .execute("notebook.create", &json!({ "title": "Intact Notes" }))
                .unwrap();
            session.save_library().unwrap();
            session
                .active_local_profile()
                .unwrap()
                .local_profile_id
                .clone()
        };

        let prof_dir = drawcraft_sync::profile_library_dir(&dir, &profile_id);
        // Simulate forced termination leaving truncated/garbage files and stale temp files.
        std::fs::write(
            dir.join(drawcraft_sync::GLOBAL_SYNC_FILE),
            b"{truncated json",
        )
        .unwrap();
        std::fs::write(
            prof_dir.join(drawcraft_sync::SYNC_MANIFEST_FILE),
            b"not json",
        )
        .unwrap();
        std::fs::write(prof_dir.join(STUDIO_PREFS_FILE), b"").unwrap();
        std::fs::write(prof_dir.join("nb_corrupt.ncraft"), b"NCFT\xff\x00bad").unwrap();
        std::fs::write(prof_dir.join("nb_empty.ncraft"), b"").unwrap();
        std::fs::write(prof_dir.join("nb_0001.ncraft.tmp.99.0"), b"stale temp").unwrap();

        let reopened = Session::open_desktop_library(dir);
        assert!(
            reopened.notebooks.iter().any(|d| d.title == "Intact Notes"),
            "valid notebooks must still load when sibling files are corrupted"
        );

        // Also test opening when NO valid notebooks survive alongside corrupt state.
        for entry in std::fs::read_dir(&prof_dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_file()
                && p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e == "dcraft" || e == "scraft" || e == "pcraft" || e == "ncraft"
                })
            {
                std::fs::write(p, b"truncated").unwrap();
            }
        }
        let empty_reopen = Session::open_desktop_library(guard.0.clone());
        assert!(empty_reopen.notebooks.is_empty());
        assert_eq!(empty_reopen.doc.page_count(), 1);
    }

    #[test]
    fn starter_documents_use_pure_white_paper_and_embedded_daily_notebook() {
        let starters = starter_documents();
        assert_eq!(starters.len(), 4);
        assert_eq!(starters[0].title, "Daily Notebook");
        assert_eq!(starters[0].page_count(), 2);
        let total_strokes: usize = starters[0]
            .resolved_pages()
            .iter()
            .map(|p| p.strokes.len())
            .sum();
        assert!(total_strokes >= 30);
        for doc in &starters {
            assert_eq!(doc.paper_color, drawcraft_doc::PaperColor::PureWhite);
            for page in doc.resolved_pages() {
                assert_eq!(page.paper_color, drawcraft_doc::PaperColor::PureWhite);
            }
        }
    }
}
