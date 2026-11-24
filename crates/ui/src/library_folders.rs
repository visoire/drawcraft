//! Library organization for the Notebooks home screen: the folder model, the
//! left navigation sidebar (Library views, folder tree, tags), subfolder tiles,
//! folder action menus, notebook drag-and-drop, and the notebook folder picker.
//!
//! Interaction model:
//! - Exactly one *place* is open at a time ([`LibraryScope`]): All Notebooks,
//!   Favorites, Unfiled, Trash, a folder, or a tag.
//! - Opening a folder shows its subfolders as tiles plus the notebooks stored
//!   directly inside it, like a file browser.
//! - Notebooks move by dragging them onto any folder (sidebar row or tile), or
//!   through the "Move to Folder" menu on every notebook.
//! - Folder actions (new subfolder, rename/move, delete) live in the `...` menu
//!   on each folder row, its right-click menu, and the open folder's header.

use drawcraft_color::{Color, contrast_ratio, relative_luminance};
use drawcraft_doc::{
    FOLDER_COLOR_PRESETS, MAX_FOLDER_DEPTH, MAX_FOLDER_PATH_CHARS, MAX_LIBRARY_FOLDERS,
    UserStudioConfig, sanitize_library_folders,
};
use egui::{
    Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2,
};
use serde_json::json;

use crate::DrawCraftApp;
use crate::icons::{Icon, compact_icon_button, paint_icon};
use crate::modal::{self, ActiveModal};
use crate::theme::{Tokens, to_color32};

/// Fixed width of the Library navigation sidebar.
pub(crate) const SIDEBAR_WIDTH: f32 = 236.0;
/// Below this window width the sidebar collapses into a "Browse" menu button.
pub(crate) const SIDEBAR_MIN_VIEWPORT: f32 = 760.0;
/// Maximum characters allowed in a single folder name.
pub(crate) const MAX_FOLDER_NAME_CHARS: usize = 48;

const ROW_HEIGHT: f32 = 36.0;
const INDENT: f32 = 14.0;
const FOLDER_TILE_HEIGHT: f32 = 148.0;

// ---------------------------------------------------------------------------
// Folder paths
// ---------------------------------------------------------------------------

/// Parent path of `path` (`""` for top-level folders).
pub(crate) fn folder_parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Last segment (display name) of `path`.
pub(crate) fn folder_leaf(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, leaf)| leaf)
}

/// Number of segments in `path` (`0` for the top level).
pub(crate) fn folder_depth(path: &str) -> usize {
    if path.is_empty() {
        0
    } else {
        path.split('/').count()
    }
}

/// Joins a parent path and a single folder name.
pub(crate) fn join_folder(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// Human-friendly path such as `School / Math`.
pub(crate) fn display_folder_path(path: &str) -> String {
    path.replace('/', " / ")
}

/// Returns `true` when `candidate` is `ancestor` itself or nested anywhere inside it.
pub(crate) fn path_within(candidate: &str, ancestor: &str) -> bool {
    if candidate.is_empty() || ancestor.is_empty() {
        return false;
    }
    if candidate.eq_ignore_ascii_case(ancestor) {
        return true;
    }
    let head_matches = candidate
        .get(..ancestor.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(ancestor));
    head_matches && candidate.as_bytes().get(ancestor.len()) == Some(&b'/')
}

/// Every folder known to the library (explicit folders plus folders referenced by
/// notebooks), parents included, sorted as a depth-first tree.
pub(crate) fn known_library_folders(app: &DrawCraftApp) -> Vec<String> {
    let mut raw = app.session.prefs.library_folders.clone();
    raw.extend(
        app.session
            .notebooks
            .iter()
            .filter(|doc| !doc.folder_path.is_empty())
            .map(|doc| doc.folder_path.clone()),
    );
    let mut folders = sanitize_library_folders(&raw);
    folders.sort_by_cached_key(|path| {
        path.split('/')
            .map(str::to_lowercase)
            .collect::<Vec<String>>()
    });
    folders
}

/// Checks a folder name typed by the user and returns the full path it would create.
///
/// `ignore` is the folder being renamed, so it never collides with itself.
pub(crate) fn validate_folder_name(
    name: &str,
    parent: &str,
    existing: &[String],
    ignore: Option<&str>,
) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Type a name for the folder.".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("Folder names can't contain / or \\ characters.".to_string());
    }
    if trimmed == "." || trimmed == ".." || trimmed.chars().any(char::is_control) {
        return Err("Choose a different folder name.".to_string());
    }
    if trimmed.chars().count() > MAX_FOLDER_NAME_CHARS {
        return Err(format!(
            "Keep folder names to {MAX_FOLDER_NAME_CHARS} characters or fewer."
        ));
    }
    if folder_depth(parent) + 1 > MAX_FOLDER_DEPTH {
        return Err(format!(
            "Folders can be nested up to {MAX_FOLDER_DEPTH} levels deep."
        ));
    }
    let path = join_folder(parent, trimmed);
    if path.chars().count() > MAX_FOLDER_PATH_CHARS {
        return Err(
            "This location is nested too deeply. Pick a shorter name or place.".to_string(),
        );
    }
    let is_ignored = |folder: &str| ignore.is_some_and(|skip| skip.eq_ignore_ascii_case(folder));
    if existing
        .iter()
        .any(|folder| folder.eq_ignore_ascii_case(&path) && !is_ignored(folder))
    {
        return Err(format!("A folder named \"{trimmed}\" already exists here."));
    }
    if ignore.is_none() && existing.len() >= MAX_LIBRARY_FOLDERS {
        return Err(format!(
            "You have reached the limit of {MAX_LIBRARY_FOLDERS} folders."
        ));
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Folder color styling (theme-safe contrast for custom folder tints)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub(crate) struct FolderVisualColors {
    pub(crate) icon: Color32,
    pub(crate) soft_fill: Color32,
    pub(crate) selected_text: Color32,
}

fn color32_to_doc(c: Color32) -> Color {
    Color::from_rgba8(c.r(), c.g(), c.b(), c.a())
}

fn ensure_contrast(color: Color, bg: Color, min_ratio: f32) -> Color {
    let c = color.sanitized().with_alpha(1.0);
    if contrast_ratio(c, bg) >= min_ratio {
        return c;
    }
    let [h, s, v] = c.to_hsv();
    let bg_is_dark = relative_luminance(bg) < 0.25;
    let eval = |t: f32| -> Color {
        if bg_is_dark {
            let next_v = v + (1.0 - v) * (t * 2.0).min(1.0);
            let desat = ((t - 0.5) * 2.0).clamp(0.0, 1.0);
            Color::from_hsv(h, s * (1.0 - desat), next_v, 1.0)
        } else {
            Color::from_hsv(h, s, v * (1.0 - t), 1.0)
        }
    };
    let mut lo = 0.0_f32;
    let mut hi = 1.0_f32;
    let mut best = eval(1.0);
    for _ in 0..14 {
        let mid = 0.5 * (lo + hi);
        let cand = eval(mid);
        if contrast_ratio(cand, bg) >= min_ratio {
            best = cand;
            hi = mid;
        } else {
            lo = mid;
        }
    }
    best
}

pub(crate) fn folder_visual_colors(custom: Option<Color>, t: &Tokens) -> FolderVisualColors {
    let Some(raw) = custom else {
        return FolderVisualColors {
            icon: t.accent,
            soft_fill: t.accent_soft,
            selected_text: t.accent,
        };
    };
    let surface = color32_to_doc(t.toolbar_bg);
    let icon_doc = ensure_contrast(raw, surface, 3.1);
    let icon = to_color32(icon_doc);
    let soft_alpha = if t.is_dark { 0.22 } else { 0.14 };
    let soft_fill = crate::motion::lerp_color(t.field_bg, icon, soft_alpha);
    let soft_doc = color32_to_doc(soft_fill);
    let text_doc = ensure_contrast(raw, soft_doc, 4.5);
    FolderVisualColors {
        icon,
        soft_fill,
        selected_text: to_color32(text_doc),
    }
}

// ---------------------------------------------------------------------------
// Folder tree model
// ---------------------------------------------------------------------------

/// One folder in the library tree with its notebook counts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FolderNode {
    pub(crate) path: String,
    /// `0` for top-level folders.
    pub(crate) depth: usize,
    /// Number of direct subfolders.
    pub(crate) subfolders: usize,
    /// Notebooks stored directly in this folder.
    pub(crate) direct_notebooks: usize,
    /// Notebooks stored in this folder or any subfolder.
    pub(crate) total_notebooks: usize,
    /// Optional custom display color assigned by the user.
    pub(crate) color: Option<Color>,
    /// Optional custom emblem icon key assigned by the user.
    pub(crate) icon: Option<String>,
    /// Whether the user starred this folder.
    pub(crate) favorite: bool,
}

impl FolderNode {
    pub(crate) fn name(&self) -> &str {
        folder_leaf(&self.path)
    }

    /// Short summary such as `3 notebooks · 1 folder`.
    pub(crate) fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.total_notebooks > 0 {
            parts.push(plural(self.total_notebooks, "notebook", "notebooks"));
        }
        if self.subfolders > 0 {
            parts.push(plural(self.subfolders, "folder", "folders"));
        }
        if parts.is_empty() {
            "Empty folder".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

pub(crate) fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// Builds the full folder tree in depth-first display order.
pub(crate) fn folder_tree(app: &DrawCraftApp) -> Vec<FolderNode> {
    let folders = known_library_folders(app);
    folders
        .iter()
        .map(|path| FolderNode {
            path: path.clone(),
            depth: folder_depth(path).saturating_sub(1),
            subfolders: folders
                .iter()
                .filter(|other| folder_parent(other).eq_ignore_ascii_case(path))
                .count(),
            direct_notebooks: app
                .session
                .notebooks
                .iter()
                .filter(|doc| doc.folder_path.eq_ignore_ascii_case(path))
                .count(),
            total_notebooks: app
                .session
                .notebooks
                .iter()
                .filter(|doc| path_within(&doc.folder_path, path))
                .count(),
            color: app.session.prefs.folder_color(path),
            icon: app.session.prefs.folder_icon(path).map(str::to_string),
            favorite: app.session.prefs.is_folder_favorite(path),
        })
        .collect()
}

/// Direct subfolders of `parent` (`""` = top level).
pub(crate) fn subfolders_of(tree: &[FolderNode], parent: &str) -> Vec<FolderNode> {
    tree.iter()
        .filter(|node| folder_parent(&node.path).eq_ignore_ascii_case(parent))
        .cloned()
        .collect()
}

#[allow(dead_code)]
fn is_collapsed(app: &DrawCraftApp, path: &str) -> bool {
    app.view
        .library_collapsed_folders
        .iter()
        .any(|collapsed| collapsed.eq_ignore_ascii_case(path))
}

#[allow(dead_code)]
fn hidden_by_collapsed_ancestor(app: &DrawCraftApp, path: &str) -> bool {
    let mut current = folder_parent(path);
    while !current.is_empty() {
        if is_collapsed(app, current) {
            return true;
        }
        current = folder_parent(current);
    }
    false
}

// ---------------------------------------------------------------------------
// Library scope (the place currently open in the library)
// ---------------------------------------------------------------------------

/// The single place currently open in the Library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LibraryScope {
    All,
    Favorites,
    Shared,
    Marketplace,
    Unfiled,
    Trash,
    Folder(String),
    Tag(String),
}

impl LibraryScope {
    pub(crate) fn motion_key(&self) -> String {
        match self {
            Self::All => "all".to_string(),
            Self::Favorites => "favorites".to_string(),
            Self::Shared => "shared".to_string(),
            Self::Marketplace => "marketplace".to_string(),
            Self::Unfiled => "Library".to_string(),
            Self::Trash => "trash".to_string(),
            Self::Folder(path) => format!("folder:{}", path.to_lowercase()),
            Self::Tag(tag) => format!("tag:{}", tag.to_lowercase()),
        }
    }

    pub(crate) fn title(&self) -> String {
        match self {
            Self::All => "Library".to_string(),
            Self::Favorites => "Favorites".to_string(),
            Self::Shared => "Shared".to_string(),
            Self::Marketplace => "Marketplace".to_string(),
            Self::Unfiled => "Library".to_string(),
            Self::Trash => "Trash".to_string(),
            Self::Folder(path) => folder_leaf(path).to_string(),
            Self::Tag(tag) => format!("#{tag}"),
        }
    }

    pub(crate) fn description(&self) -> &'static str {
        match self {
            Self::All => "Your notebooks and folders.",
            Self::Favorites => "Notebooks you starred.",
            Self::Shared => "Collaborative shared notebooks are coming soon.",
            Self::Marketplace => "Templates, covers, and study packs are coming soon.",
            Self::Unfiled => "Notebooks that are not in a folder yet.",
            Self::Trash => {
                "Deleted notebooks stay in Trash for 30 days and are removed automatically when they expire. Restore a notebook any time before then."
            }
            Self::Folder(_) => "Open a subfolder or notebook below.",
            Self::Tag(_) => "Notebooks labeled with this tag, from every folder.",
        }
    }
}

/// Reads the place currently open in the Library from view state.
pub(crate) fn current_scope(app: &DrawCraftApp) -> LibraryScope {
    if app.view.library_show_trash {
        return LibraryScope::Trash;
    }
    if app.view.library_show_shared {
        return LibraryScope::Shared;
    }
    if app.view.library_show_marketplace {
        return LibraryScope::Marketplace;
    }
    match app.view.library_folder_filter.as_deref() {
        Some("") => return LibraryScope::Unfiled,
        Some(path) => return LibraryScope::Folder(path.to_string()),
        None => {}
    }
    if app.view.library_favorites_only {
        return LibraryScope::Favorites;
    }
    if let Some(tag) = &app.view.library_tag_filter {
        return LibraryScope::Tag(tag.clone());
    }
    LibraryScope::All
}

/// Opens `scope`, replacing whichever place was open before.
pub(crate) fn set_scope(app: &mut DrawCraftApp, scope: LibraryScope) {
    app.view.library_show_trash = false;
    app.view.library_show_shared = false;
    app.view.library_show_marketplace = false;
    app.view.library_favorites_only = false;
    app.view.library_folder_filter = None;
    app.view.library_tag_filter = None;
    match scope {
        LibraryScope::All => {}
        LibraryScope::Favorites => app.view.library_favorites_only = true,
        LibraryScope::Shared => app.view.library_show_shared = true,
        LibraryScope::Marketplace => app.view.library_show_marketplace = true,
        LibraryScope::Unfiled => app.view.library_folder_filter = Some(String::new()),
        LibraryScope::Trash => app.view.library_show_trash = true,
        LibraryScope::Folder(path) => {
            // Reveal the opened folder in the sidebar tree.
            app.view.library_collapsed_folders.retain(|collapsed| {
                !path_within(&path, collapsed) || collapsed.eq_ignore_ascii_case(&path)
            });
            app.view.library_folder_filter = Some(path);
        }
        LibraryScope::Tag(tag) => app.view.library_tag_filter = Some(tag),
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Payload carried while a notebook is dragged across the Library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NotebookDragPayload {
    pub(crate) index: usize,
}

/// A deferred Library interaction, applied once drawing has finished.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LibraryAction {
    Navigate(LibraryScope),
    #[allow(dead_code)]
    ToggleCollapse(String),
    NewFolder {
        parent: String,
        move_notebook: Option<usize>,
    },
    RenameFolder(String),
    FavoriteFolder {
        path: String,
        favorite: bool,
    },
    #[allow(dead_code)]
    OpenFolderColor(String),
    #[allow(dead_code)]
    SetFolderColor {
        path: String,
        color: Option<Color>,
    },
    DeleteFolder(String),
    OpenTagColor(String),
    SetTagColor {
        tag: String,
        color: Option<Color>,
    },
    MoveNotebook {
        index: usize,
        folder: String,
    },
    FavoriteNotebook(usize),
    TrashNotebook(usize),
}

/// Applies a deferred Library interaction through engine commands and dialogs.
pub(crate) fn apply_library_action(app: &mut DrawCraftApp, action: LibraryAction) {
    match action {
        LibraryAction::Navigate(scope) => set_scope(app, scope),
        LibraryAction::ToggleCollapse(path) => {
            let before = app.view.library_collapsed_folders.len();
            app.view
                .library_collapsed_folders
                .retain(|collapsed| !collapsed.eq_ignore_ascii_case(&path));
            if app.view.library_collapsed_folders.len() == before {
                app.view.library_collapsed_folders.push(path);
            }
        }
        LibraryAction::NewFolder {
            parent,
            move_notebook,
        } => crate::folder_modals::open_new_folder(app, &parent, move_notebook),
        LibraryAction::RenameFolder(path) => crate::folder_modals::open_rename_folder(app, &path),
        LibraryAction::FavoriteFolder { path, favorite } => {
            let _ = app.session.execute(
                "library.folder.setFavorite",
                &json!({ "path": path, "favorite": favorite }),
            );
        }
        LibraryAction::OpenFolderColor(path) => {
            crate::folder_modals::open_folder_color(app, &path);
        }
        LibraryAction::SetFolderColor { path, color } => {
            let payload = match color {
                Some(c) => json!({ "path": path, "color": c }),
                None => json!({ "path": path, "clear": true }),
            };
            let _ = app.session.execute("library.folder.setColor", &payload);
        }
        LibraryAction::DeleteFolder(path) => crate::folder_modals::open_delete_folder(app, &path),
        LibraryAction::OpenTagColor(tag) => {
            crate::folder_modals::open_tag_color(app, &tag);
        }
        LibraryAction::SetTagColor { tag, color } => {
            let payload = match color {
                Some(c) => json!({ "tag": tag, "color": c }),
                None => json!({ "tag": tag, "clear": true }),
            };
            let _ = app.session.execute("library.tag.setColor", &payload);
        }
        LibraryAction::MoveNotebook { index, folder } => {
            let _ = move_notebook_to_folder(app, index, &folder);
        }
        LibraryAction::FavoriteNotebook(index) => {
            let _ = app.session.execute(
                "notebook.setMetadata",
                &json!({ "index": index, "favorite": true }),
            );
        }
        LibraryAction::TrashNotebook(index) => {
            if let Some(doc) = app.session.notebooks.get(index).cloned() {
                app.view.pending_delete_notebook = Some(doc);
                modal::open(app, ActiveModal::ConfirmDeleteNotebook);
            }
        }
    }
}

/// Moves notebook `index` into `folder` (`""` = Unfiled). Returns `true` if it moved.
pub(crate) fn move_notebook_to_folder(app: &mut DrawCraftApp, index: usize, folder: &str) -> bool {
    let Some(doc) = app.session.notebooks.get(index) else {
        return false;
    };
    if doc.folder_path == folder {
        return false;
    }
    app.session
        .execute(
            "notebook.setMetadata",
            &json!({ "index": index, "folder_path": folder }),
        )
        .is_ok()
}

// ---------------------------------------------------------------------------
// Shared drawing helpers
// ---------------------------------------------------------------------------

fn truncated_galley(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat {
            font_id: FontId::proportional(size),
            color,
            ..Default::default()
        },
    );
    job.wrap.max_width = max_width.max(8.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}

/// Full-width menu entry with a leading icon, used by folder and notebook menus.
pub(crate) fn menu_item(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    danger: bool,
    t: &Tokens,
) -> egui::Response {
    menu_item_tinted(ui, icon, None, label, enabled, danger, t)
}

fn menu_item_tinted(
    ui: &mut egui::Ui,
    icon: Icon,
    icon_color: Option<Color32>,
    label: &str,
    enabled: bool,
    danger: bool,
    t: &Tokens,
) -> egui::Response {
    let width = ui.available_width().clamp(180.0, 320.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), sense);
    if ui.is_rect_visible(rect) {
        let hovered = enabled && resp.hovered();
        let fg = if !enabled {
            t.text_muted.gamma_multiply(0.55)
        } else if danger {
            t.danger_fg
        } else {
            t.text
        };
        let icon_fg = match (icon_color, enabled) {
            (Some(c), true) => c,
            (Some(c), false) => c.gamma_multiply(0.55),
            (None, _) => fg,
        };
        if hovered {
            let fill = if danger {
                t.danger_bg
            } else {
                t.border.gamma_multiply(0.45)
            };
            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
        }
        let icon_rect =
            Rect::from_center_size(pos2(rect.left() + 15.0, rect.center().y), Vec2::splat(14.0));
        paint_icon(ui.painter(), icon_rect, icon, icon_fg);
        let galley = truncated_galley(ui, label, 12.5, fg, rect.width() - 38.0);
        ui.painter().galley(
            pos2(rect.left() + 30.0, rect.center().y - galley.size().y * 0.5),
            galley,
            fg,
        );
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

pub(crate) fn menu_separator(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(3.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, t.border));
    ui.add_space(3.0);
}

fn same_rgb(a: Color, b: Color) -> bool {
    a.to_rgba8()[..3] == b.to_rgba8()[..3]
}

#[derive(Clone, Copy)]
enum QuickColorTarget<'a> {
    Tag(&'a str),
}

impl QuickColorTarget<'_> {
    fn default_tooltip(self) -> &'static str {
        match self {
            Self::Tag(_) => "Default tag color",
        }
    }

    fn action(self, color: Option<Color>) -> LibraryAction {
        match self {
            Self::Tag(tag) => LibraryAction::SetTagColor {
                tag: tag.to_string(),
                color,
            },
        }
    }
}

fn draw_menu_quick_colors(
    ui: &mut egui::Ui,
    target: QuickColorTarget<'_>,
    current_color: Option<Color>,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(
            RichText::new("Quick Color")
                .size(10.5)
                .strong()
                .color(t.text_muted),
        );
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.spacing_mut().item_spacing.x = 2.5;
        let (def_rect, def_resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
        if ui.is_rect_visible(def_rect) {
            let p = ui.painter();
            if current_color.is_none() {
                p.circle_stroke(def_rect.center(), 9.0, Stroke::new(1.8, t.accent));
            }
            p.circle_filled(def_rect.center(), 6.8, t.panel_bg);
            p.circle_stroke(def_rect.center(), 6.8, Stroke::new(1.0, t.border));
            let offset = Vec2::splat(4.0);
            p.line_segment(
                [def_rect.center() - offset, def_rect.center() + offset],
                Stroke::new(1.1, t.text_muted),
            );
        }
        if def_resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(target.default_tooltip())
            .clicked()
        {
            actions.push(target.action(None));
            ui.close();
        }

        let rim = if t.is_dark {
            Color32::from_white_alpha(48)
        } else {
            Color32::from_black_alpha(38)
        };
        for &(name, hex) in FOLDER_COLOR_PRESETS {
            let Some(swatch) = Color::from_hex(hex) else {
                continue;
            };
            let active = current_color.is_some_and(|c| same_rgb(c, swatch));
            let (dot_rect, dot_resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
            if ui.is_rect_visible(dot_rect) {
                let p = ui.painter();
                if active {
                    p.circle_stroke(dot_rect.center(), 9.0, Stroke::new(1.8, t.accent));
                }
                p.circle_filled(dot_rect.center(), 6.8, to_color32(swatch));
                p.circle_stroke(dot_rect.center(), 6.8, Stroke::new(0.9, rim));
            }
            if dot_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("{name} ({hex})"))
                .clicked()
            {
                actions.push(target.action(Some(swatch)));
                ui.close();
            }
        }
    });
    ui.add_space(2.0);
}

/// Default studio-inspired sky-blue folder tint when the user has not chosen a custom color.
pub(crate) const DEFAULT_FOLDER_BLUE: Color32 = Color32::from_rgb(0x63, 0xB3, 0xFD);

/// Paints a crisp layered studio-style folder silhouette with optional centered emblem icon.
pub(crate) fn paint_studio_folder_icon(
    p: &egui::Painter,
    rect: Rect,
    custom_color: Option<Color>,
    icon_key: Option<&str>,
    _t: &Tokens,
) {
    let base = custom_color.map_or(DEFAULT_FOLDER_BLUE, to_color32);
    let darken = |c: Color32, f: f32| -> Color32 {
        Color32::from_rgba_unmultiplied(
            (f32::from(c.r()) * (1.0 - f)).clamp(0.0, 255.0) as u8,
            (f32::from(c.g()) * (1.0 - f)).clamp(0.0, 255.0) as u8,
            (f32::from(c.b()) * (1.0 - f)).clamp(0.0, 255.0) as u8,
            c.a(),
        )
    };
    let lighten = |c: Color32, f: f32| -> Color32 {
        Color32::from_rgba_unmultiplied(
            (f32::from(c.r()) + (255.0 - f32::from(c.r())) * f).clamp(0.0, 255.0) as u8,
            (f32::from(c.g()) + (255.0 - f32::from(c.g())) * f).clamp(0.0, 255.0) as u8,
            (f32::from(c.b()) + (255.0 - f32::from(c.b())) * f).clamp(0.0, 255.0) as u8,
            c.a(),
        )
    };

    let w = rect.width();
    let h = rect.height();
    let r_outer = (h * 0.13).clamp(6.0, 14.0).round() as u8;
    let r_tab = (h * 0.09).clamp(4.0, 10.0).round() as u8;

    let tab_color = darken(base, 0.13);
    let edge_shadow = darken(base, 0.20);
    let lip_highlight = lighten(base, 0.22);

    // 1. Subtle ground contact shadow beneath folder.
    let shadow_rect = Rect::from_min_max(
        pos2(rect.left() + w * 0.04, rect.bottom() - h * 0.12),
        pos2(rect.right() - w * 0.04, rect.bottom() + 2.5),
    );
    p.rect_filled(
        shadow_rect,
        CornerRadius::same(r_outer),
        Color32::from_black_alpha(18),
    );

    // 2. Back tab (top-left rounded folder ear + back wall).
    let tab_w = w * 0.42;
    let tab_h = h * 0.24;
    let tab_rect = Rect::from_min_size(rect.min, Vec2::new(tab_w, tab_h + h * 0.16));
    p.rect_filled(tab_rect, CornerRadius::same(r_tab), tab_color);

    let back_wall = Rect::from_min_max(
        pos2(rect.left(), rect.top() + h * 0.095),
        pos2(rect.right(), rect.bottom() - h * 0.18),
    );
    p.rect_filled(back_wall, CornerRadius::same(r_tab), tab_color);

    // 3. Front body flap with crisp bottom lip depth and subtle top highlight.
    let body_top = rect.top() + h * 0.175;
    let body_rect = Rect::from_min_max(pos2(rect.left(), body_top), rect.max);
    p.rect_filled(body_rect, CornerRadius::same(r_outer), edge_shadow);

    let face_rect = Rect::from_min_max(
        pos2(rect.left(), body_top),
        pos2(rect.right(), rect.bottom() - (h * 0.032).max(2.0)),
    );
    p.rect_filled(face_rect, CornerRadius::same(r_outer), base);

    // Soft top highlight rim along front pocket fold.
    let rim_left = face_rect.left() + f32::from(r_outer) * 0.65;
    let rim_right = face_rect.right() - f32::from(r_outer) * 0.65;
    if rim_right > rim_left {
        p.line_segment(
            [
                pos2(rim_left, face_rect.top() + 1.0),
                pos2(rim_right, face_rect.top() + 1.0),
            ],
            Stroke::new(1.2, lip_highlight.gamma_multiply(0.55)),
        );
    }

    // 4. Optional subtle emblem icon stamped cleanly onto the folder face.
    if let Some(key) = icon_key
        && let Some(icon) = crate::icons::folder_icon_from_key(key)
    {
        let lum = relative_luminance(color32_to_doc(base));
        let emblem_tint = if lum < 0.22 {
            lighten(base, 0.58)
        } else {
            darken(base, 0.42)
        };
        let icon_dim = (face_rect.height() * 0.44).clamp(16.0, 40.0);
        let icon_rect = Rect::from_center_size(face_rect.center(), Vec2::splat(icon_dim));
        paint_icon(p, icon_rect, icon, emblem_tint.gamma_multiply(0.82));
    }
}

/// Contents of a folder's action menu (chevron arrow button and right-click):
/// intentionally minimal — Edit, Favorite and Delete.
pub(crate) fn folder_menu_contents(
    ui: &mut egui::Ui,
    node: &FolderNode,
    _can_add_folder: bool,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    if menu_item(ui, Icon::Edit, "Edit Folder...", true, false, t).clicked() {
        actions.push(LibraryAction::RenameFolder(node.path.clone()));
        ui.close();
    }
    let fav_label = if node.favorite {
        "Remove from Favorites"
    } else {
        "Add to Favorites"
    };
    if menu_item(ui, Icon::Star, fav_label, true, false, t).clicked() {
        actions.push(LibraryAction::FavoriteFolder {
            path: node.path.clone(),
            favorite: !node.favorite,
        });
        ui.close();
    }
    menu_separator(ui, t);
    if menu_item(ui, Icon::Trash, "Delete Folder...", true, true, t).clicked() {
        actions.push(LibraryAction::DeleteFolder(node.path.clone()));
        ui.close();
    }
}

/// Contents of a tag's action menu (`...` button and right-click).
pub(crate) fn tag_menu_contents(
    ui: &mut egui::Ui,
    tag: &str,
    color: Option<Color>,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    if menu_item(ui, Icon::Tag, "Open", true, false, t).clicked() {
        actions.push(LibraryAction::Navigate(LibraryScope::Tag(tag.to_string())));
        ui.close();
    }
    if menu_item_tinted(
        ui,
        Icon::Palette,
        color.map(|c| folder_visual_colors(Some(c), t).icon),
        "Tag Color...",
        true,
        false,
        t,
    )
    .clicked()
    {
        actions.push(LibraryAction::OpenTagColor(tag.to_string()));
        ui.close();
    }
    menu_separator(ui, t);
    draw_menu_quick_colors(ui, QuickColorTarget::Tag(tag), color, t, actions);
}

/// Contents of a notebook's "Move to Folder" menu.
pub(crate) fn move_to_folder_menu_contents(
    ui: &mut egui::Ui,
    index: usize,
    current_folder: &str,
    tree: &[FolderNode],
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    ui.label(
        RichText::new("Move to")
            .size(10.5)
            .strong()
            .color(t.text_muted),
    );
    ui.add_space(2.0);
    egui::ScrollArea::vertical()
        .id_salt(("drawcraft_move_to_folder_scroll", index))
        .max_height(280.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let unfiled_here = current_folder.is_empty();
            if menu_item(ui, Icon::Inbox, "Library", !unfiled_here, false, t)
                .on_hover_text("Take the notebook out of every folder")
                .clicked()
            {
                actions.push(LibraryAction::MoveNotebook {
                    index,
                    folder: String::new(),
                });
            }
            for node in tree {
                let here = node.path.eq_ignore_ascii_case(current_folder);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(node.depth as f32 * INDENT);
                    let icon = if here { Icon::FolderOpen } else { Icon::Folder };
                    let label = if here {
                        format!("{} (current)", node.name())
                    } else {
                        node.name().to_string()
                    };
                    let icon_color = node.color.map(|c| folder_visual_colors(Some(c), t).icon);
                    if menu_item_tinted(ui, icon, icon_color, &label, !here, false, t).clicked() {
                        actions.push(LibraryAction::MoveNotebook {
                            index,
                            folder: node.path.clone(),
                        });
                    }
                });
            }
        });
    menu_separator(ui, t);
    if menu_item(
        ui,
        Icon::FolderPlus,
        "New Folder...",
        tree.len() < MAX_LIBRARY_FOLDERS,
        false,
        t,
    )
    .on_hover_text("Create a folder and move this notebook into it")
    .clicked()
    {
        actions.push(LibraryAction::NewFolder {
            parent: String::new(),
            move_notebook: Some(index),
        });
    }
}

/// Floating label that follows the pointer while a notebook is dragged.
pub(crate) fn paint_drag_ghost(ctx: &egui::Context, title: &str, t: &Tokens) {
    let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    egui::Area::new(egui::Id::unique("drawcraft_notebook_drag_ghost"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pos + Vec2::new(16.0, 12.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(t.field_bg)
                .stroke(Stroke::new(1.5, t.accent))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(10, 6))
                .shadow(ctx.style_of(ctx.theme()).visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.set_max_width(240.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        paint_icon(ui.painter(), icon_rect, Icon::Notebook, t.accent);
                        ui.add(
                            egui::Label::new(
                                RichText::new(title).size(12.5).strong().color(t.text),
                            )
                            .truncate(),
                        );
                    });
                    ui.label(
                        RichText::new("Drop on a folder to move it")
                            .size(10.5)
                            .color(t.text_muted),
                    );
                });
        });
}

// ---------------------------------------------------------------------------
// Sidebar navigation
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expander {
    None,
    Leaf,
    Collapsed,
    Expanded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropKind {
    None,
    Accept,
}

struct NavRowSpec<'a> {
    id: egui::Id,
    icon: Icon,
    label: &'a str,
    badge: Option<&'a str>,
    count: Option<usize>,
    depth: usize,
    selected: bool,
    expander: Expander,
    menu_tooltip: Option<&'a str>,
    drop: DropKind,
    muted: bool,
    item_color: Option<Color>,
}

struct NavRowResponse {
    row: egui::Response,
    #[allow(dead_code)]
    chevron_clicked: bool,
    more: Option<egui::Response>,
    dropped: Option<usize>,
}

struct MoreSlot {
    rect: Rect,
    radius: u8,
    id: egui::Id,
    visible: bool,
    hovered: bool,
}

fn paint_more_slot(ui: &egui::Ui, slot: MoreSlot, t: &Tokens) -> f32 {
    let vis_t = crate::motion::hover_factor(ui.ctx(), slot.id.with("vis"), slot.visible);
    let hov_t = crate::motion::hover_factor(ui.ctx(), slot.id.with("hov"), slot.hovered);
    if vis_t > 0.02 {
        let p = ui.painter();
        if hov_t > 0.02 {
            p.rect_filled(
                slot.rect,
                CornerRadius::same(slot.radius),
                t.border.gamma_multiply(0.7 * vis_t * hov_t),
            );
        }
        paint_icon(
            p,
            Rect::from_center_size(slot.rect.center(), Vec2::splat(15.0)),
            Icon::More,
            t.text.gamma_multiply(vis_t),
        );
    }
    vis_t
}

fn nav_row(ui: &mut egui::Ui, spec: &NavRowSpec<'_>, t: &Tokens) -> NavRowResponse {
    let width = ui.available_width().max(120.0);
    let row_h = crate::icons::min_target(ui.ctx(), ROW_HEIGHT);
    let (rect, row) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::click());
    let accepts_drop = spec.drop == DropKind::Accept;
    let drag_over = accepts_drop && row.dnd_hover_payload::<NotebookDragPayload>().is_some();
    let dropped = if accepts_drop {
        row.dnd_release_payload::<NotebookDragPayload>()
            .map(|payload| payload.index)
    } else {
        None
    };
    let pointer_inside = row.contains_pointer();

    let mut x = rect.left() + 6.0 + spec.depth as f32 * INDENT;
    let chevron_rect = (spec.expander != Expander::None).then(|| {
        let r = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
        x += 16.0;
        r
    });
    let icon_rect = Rect::from_center_size(pos2(x + 8.0, rect.center().y), Vec2::splat(15.0));
    x += 22.0;
    let slot_rect = Rect::from_min_max(
        pos2(rect.right() - 28.0, rect.top() + 3.0),
        pos2(rect.right() - 3.0, rect.bottom() - 3.0),
    );

    let chevron_clicked = match (chevron_rect, spec.expander) {
        (Some(r), Expander::Collapsed | Expander::Expanded) => ui
            .interact(r.expand(4.0), spec.id.with("chevron"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked(),
        _ => false,
    };

    let more_id = spec.id.with("more");
    let menu_open =
        spec.menu_tooltip.is_some() && egui::Popup::is_id_open(ui.ctx(), more_id.with("popup"));
    let show_more = spec.menu_tooltip.is_some() && (pointer_inside || menu_open);
    let more = show_more.then(|| {
        let resp = ui
            .interact(slot_rect, more_id, Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        match spec.menu_tooltip {
            Some(tip) => resp.on_hover_text(tip),
            None => resp,
        }
    });

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = pointer_inside || menu_open;
        let hover_t = crate::motion::hover_factor(
            ui.ctx(),
            spec.id.with("hov"),
            hovered && !spec.selected && !drag_over,
        );
        let select_t =
            crate::motion::hover_factor(ui.ctx(), spec.id.with("sel"), spec.selected || drag_over);
        let drag_t = crate::motion::hover_factor(ui.ctx(), spec.id.with("dnd"), drag_over);
        let vis = folder_visual_colors(spec.item_color, t);
        let selected_bg = if t.is_dark {
            t.accent_soft
        } else {
            Color32::from_rgb(0xE3, 0xEF, 0xFD)
        };
        let rest_fill = crate::motion::lerp_color(Color32::TRANSPARENT, t.ghost_hover, hover_t);
        let fill = crate::motion::lerp_color(rest_fill, selected_bg, select_t);
        let radius = CornerRadius::same(crate::theme::CONTROL_RADIUS);
        if fill.a() > 0 {
            p.rect_filled(rect, radius, fill);
        }
        if drag_t > 0.02 {
            p.rect_stroke(
                rect,
                radius,
                Stroke::new(1.5, vis.icon.gamma_multiply(drag_t)),
                StrokeKind::Inside,
            );
        }
        let rest_fg = if spec.muted { t.text_muted } else { t.text };
        let fg = if spec.selected { t.text } else { rest_fg };
        let icon_fg = if spec.selected {
            spec.item_color.map_or(t.accent, to_color32)
        } else if spec.item_color.is_some() {
            vis.icon
        } else {
            rest_fg
        };
        if let Some(r) = chevron_rect {
            let chevron_icon = match spec.expander {
                Expander::Expanded => Some(Icon::ChevronDown),
                Expander::Collapsed => Some(Icon::NextPage),
                Expander::None | Expander::Leaf => None,
            };
            if let Some(icon) = chevron_icon {
                paint_icon(p, r.shrink(1.0), icon, t.text_muted);
            }
        }
        paint_icon(p, icon_rect, spec.icon, icon_fg);

        let label_right = if spec.badge.is_some() {
            rect.right() - 44.0
        } else {
            slot_rect.left() - 4.0
        };
        let galley = truncated_galley(ui, spec.label, 13.5, fg, label_right - x);
        p.galley(pos2(x, rect.center().y - galley.size().y * 0.5), galley, fg);

        if let Some(badge) = spec.badge {
            let badge_g = ui.painter().layout_no_wrap(
                badge.to_string(),
                FontId::proportional(10.5),
                t.text_muted,
            );
            p.galley(
                pos2(
                    rect.right() - badge_g.size().x - 8.0,
                    rect.center().y - badge_g.size().y * 0.5,
                ),
                badge_g,
                t.text_muted,
            );
        }

        let more_hovered = more.as_ref().is_some_and(egui::Response::hovered) || menu_open;
        let more_vis_t = paint_more_slot(
            ui,
            MoreSlot {
                rect: slot_rect,
                radius: 5,
                id: more_id,
                visible: show_more,
                hovered: more_hovered,
            },
            t,
        );
        if let Some(count) = spec.count.filter(|c| *c > 0)
            && more_vis_t < 0.98
        {
            let count_color = t.text_muted.gamma_multiply(1.0 - more_vis_t);
            let galley = ui.painter().layout_no_wrap(
                count.to_string(),
                FontId::proportional(11.0),
                count_color,
            );
            p.galley(
                pos2(
                    slot_rect.right() - 4.0 - galley.size().x,
                    rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                count_color,
            );
        }
    }

    NavRowResponse {
        row: row.on_hover_cursor(egui::CursorIcon::PointingHand),
        chevron_clicked,
        more,
        dropped,
    }
}

fn section_header(ui: &mut egui::Ui, title: &str, add_tooltip: Option<&str>, t: &Tokens) -> bool {
    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
    let galley =
        ui.painter()
            .layout_no_wrap(title.to_string(), FontId::proportional(10.5), t.text_muted);
    ui.painter().galley(
        pos2(rect.left() + 8.0, rect.center().y - galley.size().y * 0.5),
        galley,
        t.text_muted,
    );
    let Some(tooltip) = add_tooltip else {
        return false;
    };
    let btn_rect = Rect::from_center_size(
        pos2(rect.right() - 14.0, rect.center().y),
        Vec2::splat(22.0),
    );
    let resp = ui
        .interact(
            btn_rect,
            ui.scope_id().with(("section_add", title)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip);
    let hover_t = crate::motion::hover_factor(ui.ctx(), resp.id.with("hov"), resp.hovered());
    if hover_t > 0.02 {
        ui.painter().rect_filled(
            btn_rect,
            CornerRadius::same(5),
            t.border.gamma_multiply(0.7 * hover_t),
        );
    }
    paint_icon(
        ui.painter(),
        btn_rect.shrink(4.0),
        Icon::Plus,
        crate::motion::lerp_color(t.text_muted, t.text, hover_t),
    );
    resp.clicked()
}

struct TagEntry {
    name: String,
    count: usize,
    color: Option<Color>,
}

/// Everything the navigation needs, captured up front so drawing never
/// holds a borrow of the app while actions are collected.
struct NavSnapshot {
    scope: LibraryScope,
    favorites: usize,
    trash: usize,
    tags: Vec<TagEntry>,
}

impl NavSnapshot {
    fn capture(app: &DrawCraftApp) -> Self {
        let notebooks = &app.session.notebooks;
        let tags = library_tags_with_counts(app)
            .into_iter()
            .map(|(name, count)| TagEntry {
                color: app.session.prefs.tag_color(&name),
                name,
                count,
            })
            .collect();
        Self {
            scope: current_scope(app),
            favorites: notebooks.iter().filter(|doc| doc.favorite).count(),
            trash: app.session.trashed_notebooks.len(),
            tags,
        }
    }
}

/// All tags used by notebooks with how many notebooks carry each, sorted by name.
pub(crate) fn library_tags_with_counts(app: &DrawCraftApp) -> Vec<(String, usize)> {
    let mut tags: Vec<(String, usize)> = Vec::new();
    for doc in &app.session.notebooks {
        for tag in &doc.tags {
            if let Some(entry) = tags
                .iter_mut()
                .find(|(known, _)| known.eq_ignore_ascii_case(tag))
            {
                entry.1 += 1;
            } else {
                tags.push((tag.clone(), 1));
            }
        }
    }
    tags.sort_by_key(|(tag, _)| tag.to_lowercase());
    tags
}

/// Where the navigation is being drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavMode {
    /// Docked sidebar on wide windows: full folder actions and drag-and-drop.
    Sidebar,
    /// "Browse" dropdown on narrow windows: navigation only.
    Popup,
}

/// Draws the resizable docked Library sidebar. Returns its outer rectangle.
pub(crate) fn show_sidebar_panel(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> Rect {
    let frame = egui::Frame::NONE
        .fill(t.panel_bg)
        .inner_margin(Margin::symmetric(12, 14));
    let rect = egui::Panel::left("drawcraft_home_library_sidebar")
        .resizable(true)
        .default_size(SIDEBAR_WIDTH)
        .size_range(180.0..=360.0)
        .frame(frame)
        .show(ui, |ui| {
            draw_library_nav(app, ui, t, NavMode::Sidebar);
        })
        .response
        .rect;
    ui.painter().line_segment(
        [rect.right_top(), rect.right_bottom()],
        Stroke::new(1.0, t.border),
    );
    rect
}

/// Draws the Library navigation (minimalist 4-item studio-style sidebar) and applies actions.
pub(crate) fn draw_library_nav(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    mode: NavMode,
) {
    let snapshot = NavSnapshot::capture(app);
    let mut actions = Vec::new();
    let scroll = egui::ScrollArea::vertical()
        .id_salt(("drawcraft_library_nav_scroll", mode == NavMode::Popup));
    let scroll = match mode {
        NavMode::Sidebar => scroll.auto_shrink([false, false]),
        NavMode::Popup => scroll.auto_shrink([false, true]).max_height(440.0),
    };
    scroll.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        draw_library_views(ui, &snapshot, mode, t, &mut actions);
        if !snapshot.tags.is_empty() {
            ui.add_space(16.0);
            draw_tag_section(ui, &snapshot, mode, t, &mut actions);
        }
        ui.add_space(8.0);
    });
    for action in actions {
        apply_library_action(app, action);
    }
}

fn draw_library_views(
    ui: &mut egui::Ui,
    snap: &NavSnapshot,
    mode: NavMode,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    let drop = if mode == NavMode::Sidebar {
        DropKind::Accept
    } else {
        DropKind::None
    };
    struct SidebarEntry<'a> {
        scope: LibraryScope,
        icon: Icon,
        label: &'a str,
        badge: Option<&'a str>,
        count: Option<usize>,
        tooltip: &'a str,
        drop: DropKind,
    }
    let entries = [
        SidebarEntry {
            scope: LibraryScope::All,
            icon: Icon::Folder,
            label: "Library",
            badge: None,
            count: None,
            tooltip: "Your notebooks and folders",
            drop: DropKind::None,
        },
        SidebarEntry {
            scope: LibraryScope::Favorites,
            icon: Icon::Bookmark,
            label: "Favorites",
            badge: None,
            count: (snap.favorites > 0).then_some(snap.favorites),
            tooltip: "Starred notebooks",
            drop,
        },
        SidebarEntry {
            scope: LibraryScope::Marketplace,
            icon: Icon::Marketplace,
            label: "Marketplace",
            badge: Some("Soon"),
            count: None,
            tooltip: "Templates & study packs (coming soon)",
            drop: DropKind::None,
        },
    ];
    for entry in entries {
        let spec = NavRowSpec {
            id: ui.scope_id().with(("nav_view", entry.label)),
            icon: entry.icon,
            label: entry.label,
            badge: entry.badge,
            count: entry.count,
            depth: 0,
            selected: snap.scope == entry.scope,
            expander: Expander::None,
            menu_tooltip: None,
            drop: entry.drop,
            muted: entry.badge.is_some(),
            item_color: None,
        };
        let resp = nav_row(ui, &spec, t);
        if let Some(index) = resp.dropped
            && entry.scope == LibraryScope::Favorites
        {
            actions.push(LibraryAction::FavoriteNotebook(index));
        }
        if resp.row.on_hover_text(entry.tooltip).clicked() {
            actions.push(LibraryAction::Navigate(entry.scope));
        }
    }

    if snap.trash > 0 {
        ui.add_space(12.0);
        let trash_spec = NavRowSpec {
            id: ui.scope_id().with(("nav_view", "Trash")),
            icon: Icon::Trash,
            label: "Trash",
            badge: None,
            count: Some(snap.trash),
            depth: 0,
            selected: snap.scope == LibraryScope::Trash,
            expander: Expander::None,
            menu_tooltip: None,
            drop,
            muted: true,
            item_color: None,
        };
        let resp = nav_row(ui, &trash_spec, t);
        if let Some(index) = resp.dropped {
            actions.push(LibraryAction::TrashNotebook(index));
        }
        if resp
            .row
            .on_hover_text("Deleted notebooks, kept for 30 days")
            .clicked()
        {
            actions.push(LibraryAction::Navigate(LibraryScope::Trash));
        }
    }
}

fn draw_tag_section(
    ui: &mut egui::Ui,
    snap: &NavSnapshot,
    mode: NavMode,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    let _ = section_header(ui, "TAGS", None, t);
    for entry in &snap.tags {
        draw_tag_row(ui, snap, entry, mode, t, actions);
    }
}

fn draw_tag_row(
    ui: &mut egui::Ui,
    snap: &NavSnapshot,
    entry: &TagEntry,
    mode: NavMode,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    let selected =
        matches!(&snap.scope, LibraryScope::Tag(open) if open.eq_ignore_ascii_case(&entry.name));
    let sidebar = mode == NavMode::Sidebar;
    let spec = NavRowSpec {
        id: ui.scope_id().with(("nav_tag", entry.name.to_lowercase())),
        icon: Icon::Tag,
        label: &entry.name,
        badge: None,
        count: Some(entry.count),
        depth: 0,
        selected,
        expander: Expander::None,
        menu_tooltip: sidebar.then_some("Tag actions"),
        drop: DropKind::None,
        muted: false,
        item_color: entry.color,
    };
    let resp = nav_row(ui, &spec, t);
    if let Some(more) = &resp.more {
        egui::Popup::menu(more)
            .width(226.0)
            .show(|ui| tag_menu_contents(ui, &entry.name, entry.color, t, actions));
    }
    if sidebar {
        egui::Popup::context_menu(&resp.row)
            .width(226.0)
            .show(|ui| tag_menu_contents(ui, &entry.name, entry.color, t, actions));
    }
    let more_clicked = resp.more.as_ref().is_some_and(egui::Response::clicked);
    if !more_clicked
        && resp
            .row
            .on_hover_text(format!("Show notebooks tagged #{}", entry.name))
            .clicked()
    {
        actions.push(LibraryAction::Navigate(LibraryScope::Tag(
            entry.name.clone(),
        )));
    }
}

// ---------------------------------------------------------------------------
// Breadcrumb & subfolder tiles
// ---------------------------------------------------------------------------

/// Clickable `Library > School > Math` trail above an open folder's title.
pub(crate) fn draw_breadcrumb(
    ui: &mut egui::Ui,
    path: &str,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        // Back arrow: one level up (the parent folder, or the Library root).
        let parent = path.rsplit_once('/').map(|(p, _)| p.to_string());
        let (back_rect, back) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
        paint_icon(
            ui.painter(),
            Rect::from_center_size(back_rect.center(), Vec2::splat(14.0)),
            Icon::PrevPage,
            if back.hovered() { t.text } else { t.text_muted },
        );
        if back
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Back")
            .clicked()
        {
            actions.push(LibraryAction::Navigate(match parent {
                Some(p) => LibraryScope::Folder(p),
                None => LibraryScope::All,
            }));
        }
        if crumb(ui, "Library", "root", t).clicked() {
            actions.push(LibraryAction::Navigate(LibraryScope::All));
        }
        let mut prefix = String::new();
        let segments: Vec<&str> = path.split('/').collect();
        // The open folder is the page title right below, so only its ancestors are listed here.
        let last = segments.len().saturating_sub(1);
        for (i, segment) in segments.iter().enumerate() {
            prefix = join_folder(&prefix, segment);
            if i == last {
                break;
            }
            let (sep_rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 16.0), Sense::hover());
            paint_icon(
                ui.painter(),
                Rect::from_center_size(sep_rect.center(), Vec2::splat(10.0)),
                Icon::NextPage,
                t.text_muted,
            );
            if crumb(ui, segment, &prefix, t).clicked() {
                actions.push(LibraryAction::Navigate(LibraryScope::Folder(
                    prefix.clone(),
                )));
            }
        }
    });
}

fn crumb(ui: &mut egui::Ui, text: &str, id_key: &str, t: &Tokens) -> egui::Response {
    let hov_id = ui.make_persistent_id(("drawcraft_crumb_hov", id_key));
    let prev_hovered = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(hov_id).unwrap_or(false));
    let hover_t = crate::motion::hover_factor(ui.ctx(), hov_id.with("t"), prev_hovered);
    let fg = crate::motion::lerp_color(t.text_muted, t.text, hover_t);
    let resp = ui
        .add(egui::Label::new(RichText::new(text).size(13.0).color(fg)).sense(Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() != prev_hovered {
        ui.ctx().data_mut(|d| d.insert_temp(hov_id, resp.hovered()));
    }
    if hover_t > 0.02 {
        let y = resp.rect.bottom() - 1.0;
        ui.painter().hline(
            resp.rect.x_range(),
            y,
            Stroke::new(1.0, t.text_muted.gamma_multiply(hover_t)),
        );
    }
    resp
}

/// Lays out an open folder's subfolders as tiles in the notebook grid columns.
pub(crate) fn draw_subfolder_tiles(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    nodes: &[FolderNode],
    cols: usize,
    tile_w: f32,
    gap: f32,
    t: &Tokens,
) -> Vec<Rect> {
    let can_add = known_library_folders(app).len() < MAX_LIBRARY_FOLDERS;
    let mut actions = Vec::new();
    let mut rects = Vec::new();
    for (row_idx, row) in nodes.chunks(cols.max(1)).enumerate() {
        if row_idx > 0 {
            ui.add_space(gap * 0.75);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for node in row {
                rects.push(draw_folder_tile(ui, node, tile_w, can_add, t, &mut actions));
            }
        });
    }
    for action in actions {
        apply_library_action(app, action);
    }
    rects
}

fn draw_folder_tile(
    ui: &mut egui::Ui,
    node: &FolderNode,
    tile_w: f32,
    can_add: bool,
    t: &Tokens,
    actions: &mut Vec<LibraryAction>,
) -> Rect {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(tile_w, FOLDER_TILE_HEIGHT), Sense::click());
    let id = ui
        .scope_id()
        .with(("folder_tile", node.path.to_lowercase()));
    let drag_over = resp.dnd_hover_payload::<NotebookDragPayload>().is_some();
    if let Some(payload) = resp.dnd_release_payload::<NotebookDragPayload>() {
        actions.push(LibraryAction::MoveNotebook {
            index: payload.index,
            folder: node.path.clone(),
        });
    }

    let hovered = resp.contains_pointer();
    let folder_w = (tile_w * 0.56).clamp(96.0, 122.0);
    let folder_h = (folder_w * 0.78).round();
    let folder_rect = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + 10.0 + folder_h * 0.5),
        Vec2::new(folder_w, folder_h),
    );

    let title_y = folder_rect.bottom() + 10.0;
    let title_max_w = (tile_w - 30.0).max(48.0);
    let title_galley = truncated_galley(ui, node.name(), 13.5, t.text, title_max_w);
    let chevron_dim = 14.0_f32;
    let label_row_w = title_galley.size().x + 4.0 + chevron_dim;
    let label_left = rect.center().x - label_row_w * 0.5;
    let chevron_rect = Rect::from_center_size(
        pos2(
            label_left + title_galley.size().x + 4.0 + chevron_dim * 0.5,
            title_y + title_galley.size().y * 0.5,
        ),
        Vec2::splat(22.0),
    );
    let chevron_id = id.with("edit_arrow");
    let chevron_resp = ui
        .interact(chevron_rect, chevron_id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Edit or delete folder");
    let menu_open = egui::Popup::is_id_open(ui.ctx(), chevron_id.with("popup"));

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hover_t = crate::motion::hover_factor(ui.ctx(), id.with("hov"), hovered || menu_open);
        let drag_t = crate::motion::hover_factor(ui.ctx(), id.with("dnd"), drag_over);
        if drag_t > 0.02 {
            let vis = folder_visual_colors(node.color, t);
            p.rect_filled(
                rect,
                CornerRadius::same(14),
                vis.soft_fill.gamma_multiply(drag_t),
            );
            p.rect_stroke(
                rect,
                CornerRadius::same(14),
                Stroke::new(1.5, vis.icon.gamma_multiply(drag_t)),
                StrokeKind::Inside,
            );
        }
        let lift = -2.0 * hover_t;
        paint_studio_folder_icon(
            p,
            folder_rect.translate(Vec2::new(0.0, lift)),
            node.color,
            node.icon.as_deref(),
            t,
        );

        let title_color = if hovered { t.accent } else { t.text };
        let title_g = truncated_galley(ui, node.name(), 13.5, title_color, title_max_w);
        p.galley(pos2(label_left, title_y), title_g, title_color);

        let arrow_color = if chevron_resp.hovered() || menu_open {
            t.accent_hover
        } else {
            t.accent
        };
        paint_icon(
            p,
            Rect::from_center_size(chevron_rect.center(), Vec2::splat(12.0)),
            Icon::ChevronDown,
            arrow_color,
        );

        let sub = truncated_galley(ui, &node.summary(), 11.0, t.text_muted, tile_w - 12.0);
        p.galley(
            pos2(
                rect.center().x - sub.size().x * 0.5,
                title_y + title_galley.size().y + 3.0,
            ),
            sub,
            t.text_muted,
        );
    }

    egui::Popup::menu(&chevron_resp)
        .width(196.0)
        .show(|ui| folder_menu_contents(ui, node, can_add, t, actions));
    egui::Popup::context_menu(&resp)
        .width(196.0)
        .show(|ui| folder_menu_contents(ui, node, can_add, t, actions));
    if !chevron_resp.clicked()
        && resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format!("Open {}", display_folder_path(&node.path)))
            .clicked()
    {
        actions.push(LibraryAction::Navigate(LibraryScope::Folder(
            node.path.clone(),
        )));
    }
    rect
}

// ---------------------------------------------------------------------------
// Notebook folder picker (New Notebook & Notebook Settings dialogs)
// ---------------------------------------------------------------------------

/// Mutable drafts edited by [`draw_folder_picker`].
pub(crate) struct FolderPickerDrafts<'a> {
    pub(crate) folder: &'a mut String,
    pub(crate) new_open: &'a mut bool,
    pub(crate) new_name: &'a mut String,
}

/// Applies any valid pending inline "New Folder" text if the user submits the
/// enclosing notebook dialog directly without pressing "Add" first.
pub(crate) fn commit_pending_picker_folder(
    known_folders: &[String],
    drafts: FolderPickerDrafts<'_>,
) {
    let FolderPickerDrafts {
        folder,
        new_open,
        new_name,
    } = drafts;
    if *new_open && let Ok(path) = validate_folder_name(new_name, folder, known_folders, None) {
        *folder = path;
    }
    *new_open = false;
    new_name.clear();
}

/// Folder dropdown with an inline "New Folder" field for notebook dialogs.
pub(crate) fn draw_folder_picker(
    ui: &mut egui::Ui,
    id_salt: &str,
    known_folders: &[String],
    prefs: &UserStudioConfig,
    drafts: FolderPickerDrafts<'_>,
    t: &Tokens,
) {
    let FolderPickerDrafts {
        folder,
        new_open,
        new_name,
    } = drafts;
    let mut options: Vec<String> = known_folders.to_vec();
    if !folder.is_empty() && !options.iter().any(|f| f.eq_ignore_ascii_case(folder)) {
        options.push(folder.clone());
        options.sort_by_cached_key(|path| {
            path.split('/')
                .map(str::to_lowercase)
                .collect::<Vec<String>>()
        });
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let display = if folder.is_empty() {
            "Library (no folder)".to_string()
        } else {
            display_folder_path(folder)
        };
        crate::theme::studio_combo_box(
            ui,
            id_salt,
            RichText::new(display).size(12.0).color(t.text),
            260.0,
            |ui| {
                if picker_option_row(
                    ui,
                    Icon::Inbox,
                    None,
                    "Library (no folder)",
                    0,
                    folder.is_empty(),
                    t,
                )
                .clicked()
                {
                    folder.clear();
                }
                for option in &options {
                    let depth = folder_depth(option).saturating_sub(1);
                    let active = folder.eq_ignore_ascii_case(option);
                    let icon = if active {
                        Icon::FolderOpen
                    } else {
                        Icon::Folder
                    };
                    let icon_color = prefs
                        .folder_color(option)
                        .map(|c| folder_visual_colors(Some(c), t).icon);
                    if picker_option_row(
                        ui,
                        icon,
                        icon_color,
                        folder_leaf(option),
                        depth,
                        active,
                        t,
                    )
                    .on_hover_text(display_folder_path(option))
                    .clicked()
                    {
                        *folder = option.clone();
                    }
                }
            },
        );
        if !*new_open
            && compact_icon_button(ui, Icon::FolderPlus, Some("New Folder"), false, true, t)
                .on_hover_text("Create a new folder for this notebook")
                .clicked()
        {
            *new_open = true;
            new_name.clear();
        }
    });
    let new_folder_id = ui.make_persistent_id((id_salt, "inline_new_folder"));
    crate::motion::animated_collapse(ui, new_folder_id, *new_open, 92.0, |ui| {
        draw_inline_new_folder(ui, &options, folder, new_open, new_name, t);
    });
}

fn picker_option_row(
    ui: &mut egui::Ui,
    icon: Icon,
    icon_color: Option<Color32>,
    label: &str,
    depth: usize,
    selected: bool,
    t: &Tokens,
) -> egui::Response {
    let width = ui.available_width().max(220.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, CornerRadius::same(5), t.accent_soft);
        } else if resp.hovered() {
            p.rect_filled(rect, CornerRadius::same(5), t.border.gamma_multiply(0.45));
        }
        let fg = if selected { t.accent } else { t.text };
        let icon_fg = icon_color.unwrap_or(fg);
        let x = rect.left() + 8.0 + depth as f32 * INDENT;
        let icon_rect = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
        paint_icon(p, icon_rect, icon, icon_fg);
        let max_w = rect.right() - 24.0 - (x + 20.0);
        let galley = truncated_galley(ui, label, 12.0, fg, max_w);
        p.galley(
            pos2(x + 20.0, rect.center().y - galley.size().y * 0.5),
            galley,
            fg,
        );
        if selected {
            let check = Rect::from_center_size(
                pos2(rect.right() - 12.0, rect.center().y),
                Vec2::splat(13.0),
            );
            paint_icon(p, check, Icon::Check, t.accent);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn draw_inline_new_folder(
    ui: &mut egui::Ui,
    options: &[String],
    folder: &mut String,
    new_open: &mut bool,
    new_name: &mut String,
    t: &Tokens,
) {
    let validation = validate_folder_name(new_name, folder, options, None);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let hint = if folder.is_empty() {
            "New folder name".to_string()
        } else {
            format!("New folder inside {}", folder_leaf(folder))
        };
        let edit = ui.add_sized(
            [220.0, 28.0],
            egui::TextEdit::singleline(new_name)
                .hint_text(hint)
                .char_limit(MAX_FOLDER_NAME_CHARS)
                .margin(egui::Margin::symmetric(8, 2))
                .align(egui::Align2::LEFT_CENTER),
        );
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let add = compact_icon_button(ui, Icon::Check, Some("Add"), true, validation.is_ok(), t)
            .on_hover_text("Use this new folder for the notebook");
        if (add.clicked() || enter)
            && let Ok(path) = &validation
        {
            *folder = path.clone();
            *new_open = false;
            new_name.clear();
        }
        if compact_icon_button(ui, Icon::Close, Some("Cancel"), false, true, t).clicked() {
            *new_open = false;
            new_name.clear();
        }
    });
    if let Err(message) = &validation
        && !new_name.trim().is_empty()
    {
        ui.label(RichText::new(message).size(11.0).color(t.danger_fg));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn folder_path_helpers_handle_nesting_case_and_hostile_input() {
        assert_eq!(folder_parent("School/Math/Calc"), "School/Math");
        assert_eq!(folder_parent("School"), "");
        assert_eq!(folder_leaf("School/Math"), "Math");
        assert_eq!(folder_depth(""), 0);
        assert_eq!(folder_depth("A/B/C"), 3);
        assert_eq!(join_folder("", "Work"), "Work");
        assert_eq!(join_folder("Work", "Q1"), "Work/Q1");
        assert!(path_within("school/math", "School"));
        assert!(path_within("School", "school"));
        assert!(!path_within("Schoolwork", "School"));
        assert!(!path_within("", "School"));
        assert!(!path_within("School", ""));
        // Multi-byte names never panic on slicing.
        assert!(!path_within("Ünï/x", "Ün"));
        assert!(path_within("Ünï/x", "Ünï"));
    }

    #[test]
    fn folder_name_validation_explains_every_rejection() {
        let existing = vec!["School".to_string(), "School/Math".to_string()];
        assert_eq!(
            validate_folder_name(" Physics ", "School", &existing, None),
            Ok("School/Physics".to_string())
        );
        assert!(validate_folder_name("   ", "", &existing, None).is_err());
        assert!(validate_folder_name("a/b", "", &existing, None).is_err());
        assert!(validate_folder_name("..", "", &existing, None).is_err());
        assert!(validate_folder_name(&"x".repeat(49), "", &existing, None).is_err());
        let dup = validate_folder_name("math", "School", &existing, None);
        assert!(dup.is_err_and(|msg| msg.contains("already exists")));
        // Renaming a folder to a different case of its own name is allowed.
        assert_eq!(
            validate_folder_name("MATH", "School", &existing, Some("School/Math")),
            Ok("School/MATH".to_string())
        );
        let deep = (0..MAX_FOLDER_DEPTH)
            .map(|i| format!("L{i}"))
            .collect::<Vec<_>>()
            .join("/");
        assert!(validate_folder_name("Too deep", &deep, &existing, None).is_err());
    }

    #[test]
    fn scope_round_trips_and_opening_a_folder_reveals_it_in_the_tree() {
        let mut app = DrawCraftApp::default();
        for scope in [
            LibraryScope::All,
            LibraryScope::Favorites,
            LibraryScope::Unfiled,
            LibraryScope::Trash,
            LibraryScope::Folder("School/Math".to_string()),
            LibraryScope::Tag("exam".to_string()),
        ] {
            set_scope(&mut app, scope.clone());
            assert_eq!(current_scope(&app), scope);
        }
        app.view.library_collapsed_folders = vec!["school".to_string(), "Work".to_string()];
        set_scope(&mut app, LibraryScope::Folder("School/Math".to_string()));
        assert_eq!(app.view.library_collapsed_folders, vec!["Work".to_string()]);
    }

    #[test]
    fn folder_tree_orders_depth_first_and_counts_nested_notebooks() {
        let mut app = DrawCraftApp::default();
        for (title, folder) in [
            ("Calc", "School/Math"),
            ("Bio", "School"),
            ("Plan", "School Work"),
        ] {
            app.session
                .execute(
                    "notebook.create",
                    &json!({ "title": title, "folder_path": folder }),
                )
                .unwrap();
        }
        let tree = folder_tree(&app);
        let paths: Vec<&str> = tree.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, vec!["School", "School/Math", "School Work"]);
        assert_eq!(tree[0].total_notebooks, 2);
        assert_eq!(tree[0].direct_notebooks, 1);
        assert_eq!(tree[0].subfolders, 1);
        assert_eq!(tree[1].depth, 1);
        assert_eq!(subfolders_of(&tree, "School").len(), 1);
        assert_eq!(subfolders_of(&tree, "").len(), 2);
    }

    #[test]
    fn library_actions_move_favorite_and_collapse_through_commands() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute("library.folder.create", &json!({ "path": "Work" }))
            .unwrap();
        apply_library_action(
            &mut app,
            LibraryAction::MoveNotebook {
                index: 0,
                folder: "Work".to_string(),
            },
        );
        assert_eq!(app.session.notebooks[0].folder_path, "Work");
        apply_library_action(
            &mut app,
            LibraryAction::MoveNotebook {
                index: 0,
                folder: String::new(),
            },
        );
        assert_eq!(app.session.notebooks[0].folder_path, "");
        apply_library_action(&mut app, LibraryAction::FavoriteNotebook(0));
        assert!(app.session.notebooks[0].favorite);
        // Out-of-range indices are ignored.
        apply_library_action(
            &mut app,
            LibraryAction::MoveNotebook {
                index: 99,
                folder: "Work".to_string(),
            },
        );
        apply_library_action(&mut app, LibraryAction::ToggleCollapse("Work".to_string()));
        assert!(is_collapsed(&app, "work"));
        apply_library_action(&mut app, LibraryAction::ToggleCollapse("WORK".to_string()));
        assert!(!is_collapsed(&app, "Work"));
        apply_library_action(&mut app, LibraryAction::TrashNotebook(0));
        assert_eq!(
            app.view.active_modal,
            Some(ActiveModal::ConfirmDeleteNotebook)
        );
        assert!(app.view.pending_delete_notebook.is_some());
    }

    #[test]
    fn tag_actions_and_right_click_popup_change_color_and_open_modal() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "notebook.create",
                &json!({ "title": "Study Notes", "tags": ["example", "test"] }),
            )
            .unwrap();
        let teal = Color::from_hex("#0D9488").unwrap();
        apply_library_action(
            &mut app,
            LibraryAction::SetTagColor {
                tag: "example".to_string(),
                color: Some(teal),
            },
        );
        assert_eq!(app.session.prefs.tag_color("example"), Some(teal));

        apply_library_action(
            &mut app,
            LibraryAction::SetTagColor {
                tag: "example".to_string(),
                color: None,
            },
        );
        assert_eq!(app.session.prefs.tag_color("example"), None);

        // Right-clicking a tag row in the sidebar opens the tag popup with Quick Color and Tag Color...
        let ctx = egui::Context::default();
        let viewport = Vec2::new(320.0, 600.0);
        let t = Tokens::light();
        let mut tag_pos = None;
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, viewport)),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                draw_library_nav(&mut app, ui, &t, NavMode::Sidebar);
            });
            for clipped in &out.shapes {
                if let egui::Shape::Text(text) = &clipped.shape
                    && text.galley.text() == "example"
                {
                    tag_pos = Some(text.visual_bounding_rect().center());
                }
            }
            out.drop_without_applying_deltas();
        }
        let pos = tag_pos.expect("expected 'example' tag row in sidebar");

        // Secondary click (right-click) opens the context menu popup.
        let mut popup_texts = Vec::new();
        let mut tag_color_item_pos = None;
        for (pressed, time) in [(true, 0.05), (false, 0.10), (false, 0.15)] {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if time < 0.12 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, viewport)),
                time: Some(time),
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                draw_library_nav(&mut app, ui, &t, NavMode::Sidebar);
            });
            popup_texts.clear();
            fn visit_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
                match shape {
                    egui::Shape::Text(txt) => {
                        out.push((txt.galley.text().to_string(), txt.visual_bounding_rect()));
                    }
                    egui::Shape::Vec(items) => {
                        for item in items {
                            visit_text(item, out);
                        }
                    }
                    _ => {}
                }
            }
            for clipped in &out.shapes {
                visit_text(&clipped.shape, &mut popup_texts);
            }
            if let Some((_, r)) = popup_texts.iter().find(|(txt, _)| txt == "Tag Color...") {
                tag_color_item_pos = Some(r.center());
            }
            out.drop_without_applying_deltas();
        }
        assert!(popup_texts.iter().any(|(txt, _)| txt == "Open"));
        assert!(popup_texts.iter().any(|(txt, _)| txt == "Tag Color..."));
        assert!(popup_texts.iter().any(|(txt, _)| txt == "Quick Color"));

        // Clicking "Tag Color..." opens ActiveModal::TagColor for "example" and closes the popup.
        let click_pos = tag_color_item_pos.expect("Tag Color... item rect");
        for (pressed, time) in [(true, 0.20), (false, 0.25)] {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, viewport)),
                time: Some(time),
                events: vec![
                    egui::Event::PointerMoved(click_pos),
                    egui::Event::PointerButton {
                        pos: click_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                draw_library_nav(&mut app, ui, &t, NavMode::Sidebar);
            });
            out.drop_without_applying_deltas();
        }
        assert_eq!(app.view.active_modal, Some(ActiveModal::TagColor));
        assert_eq!(app.view.tag_dialog_target, "example");
        assert!(!egui::Popup::is_any_open(&ctx));
    }
}
