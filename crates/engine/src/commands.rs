//! Command registry and JSON dispatch for headless & UI parity.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use drawcraft_color::Color;
use drawcraft_compose::{
    PdfDocumentSummary, PdfError, PdfPageRenderer, RenderedPdfPage, export_notebook_pdf,
    export_notebook_pdf_pages, inspect_pdf, render_document, render_document_svg,
    render_pdf_page_rgba, with_pdf_renderer,
};
use drawcraft_doc::{
    BUILTIN_PALETTE_CLASSIC_ID, BufferAveraging, BufferPreprocessor, ColorPalettePreset,
    CustomPaperTemplate, CustomPenPreset, CustomTemplateKind, DEFAULT_TEXT_LETTER_SPACING,
    DEFAULT_TEXT_LINE_SPACING, Document, EmbeddedPdf, EraserMode, GeometricShape, HighlighterMode,
    InkStroke, LibrarySortBy, LibraryViewMode, MAX_CUSTOM_PRESETS, MAX_CUSTOM_TEMPLATE_IMAGE_BYTES,
    MAX_CUSTOM_TEMPLATE_PDF_BYTES, MAX_CUSTOM_TEMPLATES, MAX_ELEMENT_IMAGES, MAX_ELEMENT_NAME_LEN,
    MAX_ELEMENT_SHAPES, MAX_ELEMENT_STROKES, MAX_ELEMENT_TEXTS, MAX_IMAGE_PNG_BYTES,
    MAX_LIBRARY_FOLDERS, MAX_LIBRARY_TAG_COLORS, MAX_PAGE_IMAGE_BYTES, MAX_PAGE_IMAGES,
    MAX_PAGE_SHAPES, MAX_PAGE_TEXT_CHARS, MAX_PAGE_TEXT_ITEMS, MAX_PALETTE_COLORS,
    MAX_PALETTE_PRESETS, MAX_POLYGON_VERTICES, MAX_SAVED_ELEMENTS, MAX_STYLE_FAVORITES,
    MAX_TEXT_CHARS, NotebookCover, NotebookPage, NotebookPreset, PageImage, PageOrientation,
    PageResolutionPreset, PageSelectionClipboard, PaletteTarget, PaperColor, PaperTemplate,
    PdfPageBackdrop, PenKind, PenPreset, PenSettings, PressureCurve, PressureCurvePreset,
    SavedElement, ShapeKind, ShapeLineStyle, SmoothingConfig, StabilizationPreset, StabilizerMode,
    StrokeId, StrokeSample, StyleFavorite, StylusButtonAction, StylusButtonConfig, TextAlignment,
    TextFont, TextItem, TextSpan, UiTheme, base64_decode_bytes_unbounded,
    base64_decode_bytes_with_limit, folder_path_suffix, sanitize_folder_path,
    sanitize_library_folders, sanitize_link_url, sanitize_notebook_tag, sanitize_notebook_tags,
    sanitize_notebook_title, sanitize_palette_name, sanitize_pdf_name, sanitize_preset_description,
    sanitize_preset_id, sanitize_preset_name, sanitize_template_id,
};
use drawcraft_format::{
    PackagedTrashEntry, WorkspacePackage, load_document, load_document_bytes, load_package_bytes,
    save_document, save_document_bytes, save_package_bytes,
};
use drawcraft_geom::Point;
use drawcraft_paint::eraser::erase_along_path_filtered;
use drawcraft_paint::recognize_drawn_shape;
use image::ImageDecoder;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{EngineError, MAX_NOTEBOOKS, Session, ToolKind};

const MAX_COMMAND_POINTS: usize = 100_000;
const MAX_PATH_LEN: usize = 512;

/// Metadata describing an engine command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "page.add",
        label: "Add Page",
        description: "Insert a blank page after the active page in the current notebook",
    },
    CommandSpec {
        id: "page.select",
        label: "Select Page",
        description: "Switch to a specific page index inside the active notebook",
    },
    CommandSpec {
        id: "page.duplicate",
        label: "Duplicate Page",
        description: "Duplicate the active notebook page with its ink, text, and geometric shapes",
    },
    CommandSpec {
        id: "page.remove",
        label: "Remove Page",
        description: "Delete a page from the active notebook while keeping at least one page",
    },
    CommandSpec {
        id: "page.move",
        label: "Reorder Page",
        description: "Move a page to another position inside the active notebook",
    },
    CommandSpec {
        id: "page.setBookmark",
        label: "Bookmark Page",
        description: "Mark or toggle a page as bookmarked inside the active notebook",
    },
    CommandSpec {
        id: "page.rotate",
        label: "Rotate Page",
        description: "Rotate one or more pages in 90-degree clockwise steps",
    },
    CommandSpec {
        id: "page.setSize",
        label: "Set Page Size",
        description: "Set preset dimensions, orientation, or custom width and height for one or more pages",
    },
    CommandSpec {
        id: "page.applyTemplateAll",
        label: "Apply Template to All Pages",
        description: "Apply a built-in ruling, tint, or custom template across every page in the notebook",
    },
    CommandSpec {
        id: "page.batchDuplicate",
        label: "Duplicate Selected Pages",
        description: "Duplicate one or more selected pages inside the active notebook",
    },
    CommandSpec {
        id: "page.batchRemove",
        label: "Remove Selected Pages",
        description: "Remove multiple selected pages while preserving at least one page",
    },
    CommandSpec {
        id: "page.transfer",
        label: "Transfer Pages",
        description: "Copy or move selected pages into another notebook or a new notebook",
    },
    CommandSpec {
        id: "notebook.create",
        label: "Create Notebook",
        description: "Create a new multi-page notebook from a preset or custom settings",
    },
    CommandSpec {
        id: "notebook.select",
        label: "Select Notebook",
        description: "Switch the workspace to another open notebook in the library",
    },
    CommandSpec {
        id: "notebook.rename",
        label: "Rename Notebook",
        description: "Rename the active notebook or change its cover theme",
    },
    CommandSpec {
        id: "notebook.setMetadata",
        label: "Update Notebook Metadata",
        description: "Set folder placement, favorite status, or label tags on a notebook",
    },
    CommandSpec {
        id: "notebook.merge",
        label: "Merge Notebooks",
        description: "Append every page of one notebook onto another notebook",
    },
    CommandSpec {
        id: "library.folder.create",
        label: "Create Library Folder",
        description: "Create a nested folder path in the notebook library",
    },
    CommandSpec {
        id: "library.folder.rename",
        label: "Rename Library Folder",
        description: "Rename or move a library folder and all subfolders/notebooks inside it",
    },
    CommandSpec {
        id: "library.folder.delete",
        label: "Delete Library Folder",
        description: "Remove a library folder and relocate its notebooks safely",
    },
    CommandSpec {
        id: "library.folder.setFavorite",
        label: "Favorite Library Folder",
        description: "Star or unstar a library folder so it also shows under Favorites",
    },
    CommandSpec {
        id: "library.folder.setColor",
        label: "Set Library Folder Color",
        description: "Set or clear a custom color on a library folder",
    },
    CommandSpec {
        id: "library.tag.setColor",
        label: "Set Library Tag Color",
        description: "Set or clear a custom color on a library tag",
    },
    CommandSpec {
        id: "library.setViewPrefs",
        label: "Set Library View Preferences",
        description: "Switch between Grid and List views or change library sort order",
    },
    CommandSpec {
        id: "template.saveFromImage",
        label: "Save Image Template",
        description: "Save an uploaded image as a reusable custom paper template",
    },
    CommandSpec {
        id: "template.saveFromPdf",
        label: "Save PDF Template",
        description: "Save a PDF page as a reusable custom paper template",
    },
    CommandSpec {
        id: "template.delete",
        label: "Delete Custom Template",
        description: "Remove a saved user custom paper template",
    },
    CommandSpec {
        id: "notebook.delete",
        label: "Move Notebook to Trash",
        description: "Move a notebook to Trash, where it can be restored for 30 days",
    },
    CommandSpec {
        id: "notebook.restore",
        label: "Restore Notebook",
        description: "Move a notebook out of Trash and back into the active library",
    },
    CommandSpec {
        id: "notebook.trash.cleanupExpired",
        label: "Clean Expired Trash",
        description: "Permanently remove notebooks whose 30-day recovery period has ended",
    },
    CommandSpec {
        id: "notebook.save",
        label: "Save Notebook (.dcraft)",
        description: "Save the active multi-page notebook to a .dcraft JSON document",
    },
    CommandSpec {
        id: "notebook.load",
        label: "Open Notebook (.dcraft)",
        description: "Load a .dcraft or legacy .ncraft notebook document into the workspace library",
    },
    CommandSpec {
        id: "package.export",
        label: "Export Complete Package (.dcraftpkg)",
        description: "Export all notebooks, trash, folders, templates, and preferences into a single package archive",
    },
    CommandSpec {
        id: "package.import",
        label: "Import Complete Package (.dcraftpkg)",
        description: "Restore notebooks, trash, folders, templates, and preferences from a complete package archive",
    },
    CommandSpec {
        id: "pdf.import",
        label: "Import PDF",
        description: "Import a PDF document as a new notebook or insert its pages into the active notebook",
    },
    CommandSpec {
        id: "export.png",
        label: "Export Page (PNG)",
        description: "Render the active or chosen notebook page, including text and shapes, to PNG",
    },
    CommandSpec {
        id: "export.svg",
        label: "Export Page (SVG)",
        description: "Export the active or chosen notebook page with text and shapes as SVG artwork",
    },
    CommandSpec {
        id: "export.pdf",
        label: "Export Notebook (PDF)",
        description: "Export every notebook page, including text and shapes, to a multi-page PDF",
    },
    CommandSpec {
        id: "stroke.draw",
        label: "Draw Stroke",
        description: "Stabilize, smooth, and commit a new ink stroke onto the active page",
    },
    CommandSpec {
        id: "text.insert",
        label: "Insert Text",
        description: "Add a bounded, positioned text item to the active notebook page",
    },
    CommandSpec {
        id: "text.update",
        label: "Edit Text",
        description: "Update the content and style of a positioned text item",
    },
    CommandSpec {
        id: "stroke.update",
        label: "Edit Stroke",
        description: "Update the color or width of a committed ink stroke",
    },
    CommandSpec {
        id: "shape.insert",
        label: "Insert Shape",
        description: "Add a line or geometric outline to the active notebook page",
    },
    CommandSpec {
        id: "shape.update",
        label: "Edit Shape",
        description: "Update the kind or appearance of a geometric shape",
    },
    CommandSpec {
        id: "image.insert",
        label: "Insert Image",
        description: "Decode and place a raster image onto the active notebook page",
    },
    CommandSpec {
        id: "image.update",
        label: "Edit Image",
        description: "Update the position, size, or rotation of a placed page image",
    },
    CommandSpec {
        id: "selection.transform",
        label: "Transform Selection",
        description: "Move, rotate, or resize selected page objects",
    },
    CommandSpec {
        id: "selection.update",
        label: "Edit Selection",
        description: "Apply shared color or shape format to selected page objects",
    },
    CommandSpec {
        id: "selection.delete",
        label: "Delete Selection",
        description: "Remove selected strokes, text items, shapes, and images from the active page",
    },
    CommandSpec {
        id: "stroke.erase",
        label: "Erase Strokes",
        description: "Run Stroke or Precision eraser along a point path",
    },
    CommandSpec {
        id: "edit.undo",
        label: "Undo",
        description: "Revert the most recent document edit",
    },
    CommandSpec {
        id: "edit.redo",
        label: "Redo",
        description: "Re-apply the most recently undone edit",
    },
    CommandSpec {
        id: "page.clear",
        label: "Clear Page",
        description: "Remove all ink, text, images, and geometric shapes from the active page",
    },
    CommandSpec {
        id: "page.setTemplate",
        label: "Set Paper Template",
        description: "Update the page ruling template and/or paper tint",
    },
    CommandSpec {
        id: "tool.select",
        label: "Select Tool",
        description: "Switch between the Select arrow, FountainPen, BallPen, BrushPen, Highlighter, and Eraser",
    },
    CommandSpec {
        id: "pen.setSettings",
        label: "Update Pen Settings",
        description: "Configure size, color, opacity, nib geometry, pressure, or taper for a pen",
    },
    CommandSpec {
        id: "pen.setSmoothing",
        label: "Update Input Stabilization",
        description: "Select a curated stroke smoothing preset or stabilization configuration for a pen",
    },
    CommandSpec {
        id: "pen.setPressureCurve",
        label: "Update Pressure Curve",
        description: "Apply a pressure curve preset or custom control points",
    },
    CommandSpec {
        id: "pen.applyPreset",
        label: "Apply Pen Preset",
        description: "Select and apply a curated built-in pen preset",
    },
    CommandSpec {
        id: "eraser.setSettings",
        label: "Update Eraser Settings",
        description: "Set eraser mode (StrokeEraser/PrecisionEraser) and diameter",
    },
    CommandSpec {
        id: "highlighter.setSettings",
        label: "Update Highlighter Settings",
        description: "Set highlighter drawing mode (Freehand/Straight)",
    },
    CommandSpec {
        id: "palette.setWildcard",
        label: "Set Custom Color",
        description: "Update the persistent standalone custom (wildcard) color for ink or highlighter",
    },
    CommandSpec {
        id: "palette.setColor",
        label: "Set Palette Color",
        description: "Update an ink or highlighter palette swatch or add a new custom color",
    },
    CommandSpec {
        id: "palette.removeColor",
        label: "Remove Palette Color",
        description: "Remove a swatch slot from the ink or highlighter quick palette",
    },
    CommandSpec {
        id: "palette.reset",
        label: "Reset Color Palette",
        description: "Restore default curated ink and/or highlighter color swatches",
    },
    CommandSpec {
        id: "palette.selectPreset",
        label: "Select Color Palette",
        description: "Switch active toolbar and picker swatches to a named color palette group",
    },
    CommandSpec {
        id: "palette.savePreset",
        label: "Save Color Palette",
        description: "Create a new custom color palette group or rename/update an existing one",
    },
    CommandSpec {
        id: "palette.deletePreset",
        label: "Delete Color Palette",
        description: "Delete a user-created color palette group (built-in defaults are protected)",
    },
    CommandSpec {
        id: "preset.save",
        label: "Save Custom Preset",
        description: "Create or update a user-defined custom pen preset",
    },
    CommandSpec {
        id: "preset.applyCustom",
        label: "Apply Custom Preset",
        description: "Select and apply a saved user custom pen preset",
    },
    CommandSpec {
        id: "preset.delete",
        label: "Delete Custom Preset",
        description: "Remove a saved user custom pen preset",
    },
    CommandSpec {
        id: "stroke.recognizeShape",
        label: "Recognize Drawn Shape",
        description: "Convert a freehand circle, ellipse, rectangle, triangle, arrow, or polygon stroke into a crisp geometric shape",
    },
    CommandSpec {
        id: "text.toggleCheckbox",
        label: "Toggle Checklist Checkbox",
        description: "Toggle a checkbox paragraph inside a text box between unchecked and checked",
    },
    CommandSpec {
        id: "selection.copy",
        label: "Copy or Cut Selection",
        description: "Copy or cut any selected strokes, shapes, text boxes, and images to the workspace clipboard",
    },
    CommandSpec {
        id: "selection.paste",
        label: "Paste Selection",
        description: "Paste copied objects onto the current notebook page",
    },
    CommandSpec {
        id: "selection.duplicate",
        label: "Duplicate Selection",
        description: "Duplicate any selected objects in place with a clean offset",
    },
    CommandSpec {
        id: "element.save",
        label: "Save Element / Sticker",
        description: "Save selected strokes, shapes, text, and images as a reusable sticker or element",
    },
    CommandSpec {
        id: "element.place",
        label: "Place Element / Sticker",
        description: "Stamp a built-in or user-saved sticker/element onto the active notebook page",
    },
    CommandSpec {
        id: "element.delete",
        label: "Delete Saved Element",
        description: "Remove a user-saved sticker or element from the library",
    },
    CommandSpec {
        id: "styleFavorite.save",
        label: "Save Toolbar Style Favorite",
        description: "Pin the active pen tool, color, and stroke thickness as a quick toolbar favorite",
    },
    CommandSpec {
        id: "styleFavorite.apply",
        label: "Apply Toolbar Style Favorite",
        description: "Switch to a saved toolbar favorite tool, color, and stroke thickness",
    },
    CommandSpec {
        id: "styleFavorite.delete",
        label: "Delete Toolbar Style Favorite",
        description: "Remove a pinned toolbar style favorite",
    },
    CommandSpec {
        id: "theme.set",
        label: "Set UI Theme, Scale & Contrast",
        description: "Switch between Light and Dark studio themes, adjust interface scale, or toggle automatic canvas color contrast",
    },
    CommandSpec {
        id: "stylus.setButtons",
        label: "Configure Stylus Buttons",
        description: "Assign shortcut actions to connected stylus side buttons and eraser end, or reset them to defaults",
    },
    CommandSpec {
        id: "sync.status",
        label: "Get Sync Status",
        description: "Return the current account session, server URL, and pending synchronization count",
    },
    CommandSpec {
        id: "sync.setServerUrl",
        label: "Set Sync Server URL",
        description: "Update the configured self-hosted server URL for future authentication and synchronization",
    },
    CommandSpec {
        id: "sync.signOut",
        label: "Sign Out Local Session",
        description: "Flush local changes and return the session to the active local profile",
    },
    CommandSpec {
        id: "sync.transferNotebook",
        label: "Transfer Local Notebook to Cloud",
        description: "Move or copy an individual notebook from a local profile into the connected cloud account",
    },
    CommandSpec {
        id: "sync.migrateLocalAccount",
        label: "Migrate Local Account to Cloud",
        description: "Move all notebooks and data from a local account into the signed-in account and delete the old local account",
    },
    CommandSpec {
        id: "profile.completeFirstAccess",
        label: "Complete First-Access Local Nickname",
        description: "Set the local user nickname on initial application launch",
    },
    CommandSpec {
        id: "profile.create",
        label: "Create Local Profile",
        description: "Create a new isolated local-only profile on this device",
    },
    CommandSpec {
        id: "profile.rename",
        label: "Rename Local Profile",
        description: "Rename an existing local profile without affecting its notebooks",
    },
    CommandSpec {
        id: "profile.switch",
        label: "Switch Active Workspace",
        description: "Switch between a local profile and the connected cloud account without mixing data",
    },
    CommandSpec {
        id: "profile.delete",
        label: "Delete Local Profile",
        description: "Remove a non-last local profile from this device",
    },
    CommandSpec {
        id: "studyTape.add",
        label: "Add Study Tape",
        description: "Place a hidden study tape over a rectangle of the active page",
    },
    CommandSpec {
        id: "studyTape.reveal",
        label: "Reveal Study Tape",
        description: "Reveal or re-hide a study tape on the active page (progress, not an undo step)",
    },
    CommandSpec {
        id: "studyTape.remove",
        label: "Remove Study Tape",
        description: "Delete a study tape from the active page",
    },
    CommandSpec {
        id: "flashcard.add",
        label: "Add Flashcard",
        description: "Create a spaced-repetition flashcard, optionally linked to a source page",
    },
    CommandSpec {
        id: "flashcard.remove",
        label: "Remove Flashcard",
        description: "Delete a flashcard from the notebook deck",
    },
    CommandSpec {
        id: "flashcard.review",
        label: "Review Flashcard",
        description: "Grade a flashcard (Again, Hard, Good, Easy) and reschedule it",
    },
    CommandSpec {
        id: "prefs.setStudy",
        label: "Set Study Preferences",
        description: "Set left-handed layout, Pencil-only vs finger drawing, and PDF dark reading",
    },
    CommandSpec {
        id: "flashcard.due",
        label: "List Due Flashcards",
        description: "List flashcards due for review, earliest first",
    },
];

pub fn specs() -> &'static [CommandSpec] {
    COMMANDS
}

pub(crate) fn invalid(id: &str, reason: impl Into<String>) -> EngineError {
    EngineError::InvalidParams {
        command: id.to_string(),
        reason: reason.into(),
    }
}

pub(crate) fn parse_params<T: for<'de> Deserialize<'de>>(
    id: &str,
    params: &Value,
) -> Result<T, EngineError> {
    serde_json::from_value(params.clone()).map_err(|e| invalid(id, e.to_string()))
}

#[derive(Deserialize)]
struct DrawParams {
    points: Vec<StrokeSample>,
    kind: Option<PenKind>,
    pen: Option<PenSettings>,
}

#[derive(Deserialize)]
struct InsertTextParams {
    text: String,
    x: f64,
    y: f64,
    size: Option<f32>,
    #[serde(default)]
    angle_degrees: Option<f64>,
    color: Option<Color>,
    font: Option<TextFont>,
    line_spacing: Option<f32>,
    letter_spacing: Option<f32>,
    #[serde(default)]
    max_width: Option<f32>,
    #[serde(default)]
    link_url: Option<String>,
    #[serde(default)]
    alignment: Option<TextAlignment>,
    #[serde(default)]
    spans: Option<Vec<TextSpan>>,
}

#[derive(Deserialize)]
struct UpdateTextParams {
    index: usize,
    text: Option<String>,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    size: Option<f32>,
    color: Option<Color>,
    font: Option<TextFont>,
    line_spacing: Option<f32>,
    letter_spacing: Option<f32>,
    #[serde(default)]
    max_width: Option<f32>,
    #[serde(default)]
    clear_max_width: Option<bool>,
    #[serde(default)]
    link_url: Option<String>,
    #[serde(default)]
    clear_link_url: Option<bool>,
    #[serde(default)]
    alignment: Option<TextAlignment>,
    #[serde(default)]
    spans: Option<Vec<TextSpan>>,
    #[serde(default)]
    record_history: Option<bool>,
}

#[derive(Deserialize)]
struct UpdateStrokeParams {
    id: StrokeId,
    size: Option<f32>,
    color: Option<Color>,
    #[serde(default)]
    record_history: Option<bool>,
}

#[derive(Deserialize)]
struct InsertShapeParams {
    kind: ShapeKind,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    angle_degrees: Option<f64>,
    curve: Option<f64>,
    width: Option<f32>,
    color: Option<Color>,
    #[serde(default)]
    fill_color: Option<Color>,
    #[serde(default)]
    line_style: Option<ShapeLineStyle>,
    #[serde(default)]
    points: Option<Vec<Point>>,
}

#[derive(Deserialize)]
struct UpdateShapeParams {
    index: usize,
    kind: Option<ShapeKind>,
    angle_degrees: Option<f64>,
    curve: Option<f64>,
    width: Option<f32>,
    color: Option<Color>,
    #[serde(default)]
    fill_color: Option<Color>,
    #[serde(default)]
    clear_fill: Option<bool>,
    #[serde(default)]
    line_style: Option<ShapeLineStyle>,
    #[serde(default)]
    record_history: Option<bool>,
}

#[derive(Deserialize)]
struct InsertImageParams {
    #[serde(default)]
    bytes: Option<Vec<u8>>,
    #[serde(default)]
    base64_bytes: Option<String>,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    width: Option<f64>,
    #[serde(default)]
    height: Option<f64>,
    #[serde(default)]
    angle_degrees: Option<f64>,
}

#[derive(Deserialize)]
struct UpdateImageParams {
    index: usize,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    width: Option<f64>,
    #[serde(default)]
    height: Option<f64>,
    #[serde(default)]
    angle_degrees: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TransformTarget {
    Stroke { id: StrokeId },
    Shape { index: usize },
    Text { index: usize },
    Image { index: usize },
}

#[derive(Deserialize)]
struct DeleteSelectionParams {
    targets: Vec<TransformTarget>,
}

fn default_transform_scale() -> f64 {
    1.0
}

#[derive(Deserialize)]
struct TransformSelectionParams {
    targets: Vec<TransformTarget>,
    pivot: Point,
    translation: Point,
    #[serde(default = "default_transform_scale")]
    scale: f64,
    #[serde(default)]
    scale_x: Option<f64>,
    #[serde(default)]
    scale_y: Option<f64>,
    rotation_degrees: f64,
    record_history: Option<bool>,
}

#[derive(Deserialize)]
struct UpdateSelectionParams {
    targets: Vec<TransformTarget>,
    color: Option<Color>,
    shape_kind: Option<ShapeKind>,
    #[serde(default)]
    fill_color: Option<Color>,
    #[serde(default)]
    clear_fill: Option<bool>,
    #[serde(default)]
    line_style: Option<ShapeLineStyle>,
}

#[derive(Deserialize)]
struct ErasePoint {
    x: f64,
    y: f64,
    radius: Option<f64>,
}

#[derive(Deserialize)]
struct EraseParams {
    path: Vec<ErasePoint>,
    mode: Option<EraserMode>,
    radius: Option<f64>,
    #[serde(default)]
    highlighter_only: Option<bool>,
}

#[derive(Deserialize)]
struct SetTemplateParams {
    template: Option<PaperTemplate>,
    color: Option<PaperColor>,
    #[serde(default)]
    custom_template_id: Option<String>,
    #[serde(default)]
    clear_custom_template: Option<bool>,
}

#[derive(Deserialize)]
struct SelectToolParams {
    tool: String,
}

#[derive(Deserialize)]
struct SetPenSettingsParams {
    kind: Option<PenKind>,
    settings: Option<PenSettings>,
    color: Option<Color>,
    color_hex: Option<String>,
    size: Option<f32>,
    opacity: Option<f32>,
    hardness: Option<f32>,
    pressure_enabled: Option<bool>,
    pressure_size_min: Option<f32>,
    pressure_opacity_min: Option<f32>,
    velocity_pressure_simulation: Option<bool>,
    tilt_sensitivity: Option<f32>,
    nib_flatness: Option<f32>,
    nib_angle_deg: Option<f32>,
    start_taper_px: Option<f32>,
    end_taper_px: Option<f32>,
    #[serde(default)]
    persist_size: Option<bool>,
}

#[derive(Deserialize)]
struct SetSmoothingParams {
    kind: Option<PenKind>,
    preset: Option<StabilizationPreset>,
    smoothing: Option<SmoothingConfig>,
    stabilizer_mode: Option<StabilizerMode>,
    stabilizer_strength: Option<f32>,
    catmull_rom_subdivision: Option<bool>,
    spline_tension: Option<f32>,
    adjust_for_zoom: Option<bool>,
    catch_up: Option<bool>,
    catch_up_on_end: Option<bool>,
    streamline_amount: Option<f32>,
    prediction_ms: Option<f32>,
    buffer_averaging: Option<BufferAveraging>,
    buffer_preprocessor: Option<BufferPreprocessor>,
    buffer_size: Option<u16>,
    buffer_sigma: Option<f32>,
    buffer_deadzone_radius: Option<f32>,
    buffer_cusp_detection: Option<bool>,
    buffer_drag: Option<f32>,
    buffer_mass: Option<f32>,
    buffer_finalize_stroke: Option<bool>,
}

#[derive(Deserialize)]
struct SetPressureCurveParams {
    kind: Option<PenKind>,
    preset: Option<PressureCurvePreset>,
    points: Option<Vec<[f32; 2]>>,
}

#[derive(Deserialize)]
struct ApplyPenPresetParams {
    preset: PenPreset,
    preserve_color: Option<bool>,
}

#[derive(Deserialize)]
struct SetEraserParams {
    mode: Option<EraserMode>,
    size: Option<f32>,
    #[serde(default)]
    highlighter_only: Option<bool>,
}

#[derive(Deserialize)]
struct SetHighlighterParams {
    mode: Option<HighlighterMode>,
}

#[derive(Deserialize)]
struct SetWildcardColorParams {
    target: Option<PaletteTarget>,
    color: Option<Color>,
    color_hex: Option<String>,
    apply_to_pen: Option<bool>,
}

#[derive(Deserialize)]
struct SetPaletteColorParams {
    target: Option<PaletteTarget>,
    preset_id: Option<String>,
    index: Option<usize>,
    color: Option<Color>,
    color_hex: Option<String>,
    apply_to_pen: Option<bool>,
}

#[derive(Deserialize)]
struct RemovePaletteColorParams {
    target: Option<PaletteTarget>,
    preset_id: Option<String>,
    index: usize,
}

#[derive(Default, Deserialize)]
struct ResetPaletteParams {
    target: Option<String>,
    id: Option<String>,
}

#[derive(Deserialize)]
struct SelectPalettePresetParams {
    id: String,
}

#[derive(Deserialize)]
struct SavePalettePresetParams {
    id: Option<String>,
    name: String,
    ink_colors: Option<Vec<Color>>,
    highlighter_colors: Option<Vec<Color>>,
    copy_from_active: Option<bool>,
    copy_from_id: Option<String>,
    seed_color: Option<Color>,
    seed_color_hex: Option<String>,
    select: Option<bool>,
}

#[derive(Deserialize)]
struct DeletePalettePresetParams {
    id: String,
}

#[derive(Deserialize)]
struct SaveCustomPresetParams {
    id: Option<String>,
    name: String,
    description: Option<String>,
    settings: PenSettings,
    include_color: Option<bool>,
    apply_now: Option<bool>,
}

#[derive(Deserialize)]
struct ApplyCustomPresetParams {
    id: String,
    preserve_color: Option<bool>,
}

#[derive(Deserialize)]
struct DeleteCustomPresetParams {
    id: String,
}

#[derive(Deserialize)]
struct SetThemeParams {
    theme: Option<UiTheme>,
    auto_contrast_with_theme: Option<bool>,
    ui_scale: Option<f32>,
}

#[derive(Default, Deserialize)]
struct AddPageParams {
    template: Option<PaperTemplate>,
    color: Option<PaperColor>,
    #[serde(default)]
    custom_template_id: Option<String>,
    #[serde(default)]
    resolution: Option<PageResolutionPreset>,
    #[serde(default)]
    orientation: Option<PageOrientation>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[derive(Deserialize)]
struct SelectIndexParams {
    index: usize,
}

#[derive(Default, Deserialize)]
struct OptionalIndexParams {
    index: Option<usize>,
}

#[derive(Deserialize)]
struct MovePageParams {
    from: usize,
    to: usize,
}

#[derive(Default, Deserialize)]
struct BookmarkPageParams {
    index: Option<usize>,
    bookmarked: Option<bool>,
}

#[derive(Deserialize)]
struct RotatePageParams {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    pages: Option<Vec<usize>>,
    quarter_turns: i32,
}

#[derive(Default, Deserialize)]
struct SetPageSizeParams {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    pages: Option<Vec<usize>>,
    #[serde(default)]
    apply_all: Option<bool>,
    #[serde(default)]
    resolution: Option<PageResolutionPreset>,
    #[serde(default)]
    orientation: Option<PageOrientation>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[derive(Default, Deserialize)]
struct ApplyTemplateAllParams {
    template: Option<PaperTemplate>,
    color: Option<PaperColor>,
    custom_template_id: Option<String>,
    clear_custom_template: Option<bool>,
}

#[derive(Deserialize)]
struct BatchPagesParams {
    indices: Vec<usize>,
}

#[derive(Deserialize)]
struct TransferPagesParams {
    pages: Vec<usize>,
    #[serde(default)]
    target_notebook: Option<usize>,
    #[serde(default)]
    new_notebook_title: Option<String>,
    #[serde(default)]
    move_pages: Option<bool>,
}

#[derive(Default, Deserialize)]
struct CreateNotebookParams {
    preset: Option<NotebookPreset>,
    title: Option<String>,
    cover: Option<NotebookCover>,
    cover_hex: Option<String>,
    template: Option<PaperTemplate>,
    paper_color: Option<PaperColor>,
    #[serde(default)]
    resolution: Option<PageResolutionPreset>,
    #[serde(default)]
    orientation: Option<PageOrientation>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    folder_path: Option<String>,
    #[serde(default)]
    favorite: Option<bool>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    custom_template_id: Option<String>,
}

#[derive(Default, Deserialize)]
struct RenameNotebookParams {
    title: Option<String>,
    cover: Option<NotebookCover>,
    cover_hex: Option<String>,
}

#[derive(Default, Deserialize)]
struct SetNotebookMetadataParams {
    index: Option<usize>,
    folder_path: Option<String>,
    favorite: Option<bool>,
    tags: Option<Vec<String>>,
    add_tag: Option<String>,
    remove_tag: Option<String>,
}

#[derive(Deserialize)]
struct MergeNotebooksParams {
    source_index: usize,
    target_index: usize,
    #[serde(default)]
    delete_source: Option<bool>,
}

#[derive(Deserialize)]
struct CreateFolderParams {
    path: String,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    color_hex: Option<String>,
    #[serde(default)]
    icon: Option<String>,
}

#[derive(Deserialize)]
struct RenameFolderParams {
    old_path: String,
    new_path: String,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    color_hex: Option<String>,
    #[serde(default)]
    clear_color: Option<bool>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    clear_icon: Option<bool>,
}

#[derive(Deserialize)]
struct DeleteFolderParams {
    path: String,
    #[serde(default)]
    move_notebooks_to: Option<String>,
}

#[derive(Deserialize)]
struct SetFolderFavoriteParams {
    path: String,
    favorite: bool,
}

#[derive(Deserialize)]
struct SetFolderColorParams {
    path: String,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    color_hex: Option<String>,
    #[serde(default)]
    clear: Option<bool>,
}

#[derive(Deserialize)]
struct SetTagColorParams {
    tag: String,
    #[serde(default)]
    color: Option<Color>,
    #[serde(default)]
    color_hex: Option<String>,
    #[serde(default)]
    clear: Option<bool>,
}

#[derive(Default, Deserialize)]
struct SetLibraryViewPrefsParams {
    view_mode: Option<LibraryViewMode>,
    sort_by: Option<LibrarySortBy>,
    sort_ascending: Option<bool>,
}

#[derive(Deserialize)]
struct SaveImageTemplateParams {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    bytes: Option<Vec<u8>>,
    #[serde(default)]
    base64_bytes: Option<String>,
}

#[derive(Deserialize)]
struct SavePdfTemplateParams {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    bytes: Option<Vec<u8>>,
    #[serde(default)]
    base64_bytes: Option<String>,
    #[serde(default)]
    page_index: Option<usize>,
}

#[derive(Deserialize)]
struct DeleteCustomTemplateParams {
    id: String,
}

#[derive(Default, Deserialize)]
struct SaveOrExportParams {
    path: Option<String>,
    page: Option<usize>,
    #[serde(default)]
    pages: Option<Vec<usize>>,
    in_memory: Option<bool>,
}

/// Target placement when importing a PDF file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfImportMode {
    #[default]
    NewNotebook,
    InsertPages,
}

#[derive(Default, Deserialize)]
struct LoadNotebookParams {
    path: Option<String>,
    json: Option<String>,
    bytes: Option<Vec<u8>>,
    source_name: Option<String>,
    replace_active: Option<bool>,
}

#[derive(Default, Deserialize)]
struct PdfImportParams {
    path: Option<String>,
    bytes: Option<Vec<u8>>,
    base64_bytes: Option<String>,
    title: Option<String>,
    source_name: Option<String>,
    mode: Option<PdfImportMode>,
    after_page: Option<usize>,
    replace_active: Option<bool>,
}

pub(crate) fn parse_optional_params<T: Default + for<'de> Deserialize<'de>>(
    id: &str,
    params: &Value,
) -> Result<T, EngineError> {
    if params.is_null() {
        return Ok(T::default());
    }
    parse_params(id, params)
}

pub fn execute(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let prev_rev = session.revision;
    let result = match id {
        "stroke.draw" => cmd_stroke_draw(session, id, params),
        "text.insert" => cmd_text_insert(session, id, params),
        "text.update" => cmd_text_update(session, id, params),
        "stroke.update" => cmd_stroke_update(session, id, params),
        "shape.insert" => cmd_shape_insert(session, id, params),
        "shape.update" => cmd_shape_update(session, id, params),
        "image.insert" => cmd_image_insert(session, id, params),
        "image.update" => cmd_image_update(session, id, params),
        "selection.transform" => cmd_transform_selection(session, id, params),
        "selection.update" => cmd_update_selection(session, id, params),
        "selection.delete" => cmd_delete_selection(session, id, params),
        "stroke.erase" => cmd_stroke_erase(session, id, params),
        "stroke.recognizeShape" => cmd_stroke_recognize_shape(session, id, params),
        "text.toggleCheckbox" => cmd_text_toggle_checkbox(session, id, params),
        "selection.copy" => cmd_selection_copy(session, id, params),
        "selection.paste" => cmd_selection_paste(session, id, params),
        "selection.duplicate" => cmd_selection_duplicate(session, id, params),
        "edit.undo" => cmd_undo(session),
        "edit.redo" => cmd_redo(session),
        "page.clear" => cmd_page_clear(session),
        "page.setTemplate" => cmd_set_template(session, id, params),
        "page.add" => cmd_page_add(session, id, params),
        "page.select" => cmd_page_select(session, id, params),
        "page.duplicate" => cmd_page_duplicate(session, id),
        "page.remove" => cmd_page_remove(session, id, params),
        "page.move" => cmd_page_move(session, id, params),
        "page.setBookmark" => cmd_page_set_bookmark(session, id, params),
        "page.rotate" => cmd_page_rotate(session, id, params),
        "page.setSize" => cmd_page_set_size(session, id, params),
        "page.applyTemplateAll" => cmd_page_apply_template_all(session, id, params),
        "page.batchDuplicate" => cmd_page_batch_duplicate(session, id, params),
        "page.batchRemove" => cmd_page_batch_remove(session, id, params),
        "page.transfer" => cmd_page_transfer(session, id, params),
        "notebook.create" => cmd_notebook_create(session, id, params),
        "notebook.select" => cmd_notebook_select(session, id, params),
        "notebook.rename" => cmd_notebook_rename(session, id, params),
        "notebook.setMetadata" => cmd_notebook_set_metadata(session, id, params),
        "notebook.merge" => cmd_notebook_merge(session, id, params),
        "notebook.delete" => cmd_notebook_delete(session, id, params),
        "notebook.restore" => cmd_notebook_restore(session, id, params),
        "notebook.trash.cleanupExpired" => cmd_notebook_trash_cleanup(session),
        "notebook.save" => cmd_notebook_save(session, id, params),
        "notebook.load" => cmd_notebook_load(session, id, params),
        "package.export" => cmd_package_export(session, id, params),
        "package.import" => cmd_package_import(session, id, params),
        "library.folder.create" => cmd_library_folder_create(session, id, params),
        "library.folder.rename" => cmd_library_folder_rename(session, id, params),
        "library.folder.delete" => cmd_library_folder_delete(session, id, params),
        "library.folder.setColor" => cmd_library_folder_set_color(session, id, params),
        "library.folder.setFavorite" => cmd_library_folder_set_favorite(session, id, params),
        "library.tag.setColor" => cmd_library_tag_set_color(session, id, params),
        "library.setViewPrefs" => cmd_library_set_view_prefs(session, id, params),
        "template.saveFromImage" => cmd_template_save_from_image(session, id, params),
        "template.saveFromPdf" => cmd_template_save_from_pdf(session, id, params),
        "template.delete" => cmd_template_delete(session, id, params),
        "pdf.import" => cmd_pdf_import(session, id, params),
        "export.png" => cmd_export_png(session, id, params),
        "export.svg" => cmd_export_svg(session, id, params),
        "export.pdf" => cmd_export_pdf(session, id, params),
        "tool.select" => cmd_select_tool(session, id, params),
        "pen.setSettings" => cmd_set_pen_settings(session, id, params),
        "pen.setSmoothing" => cmd_set_smoothing(session, id, params),
        "pen.setPressureCurve" => cmd_set_pressure_curve(session, id, params),
        "pen.applyPreset" => cmd_apply_pen_preset(session, id, params),
        "eraser.setSettings" => cmd_set_eraser(session, id, params),
        "highlighter.setSettings" => cmd_set_highlighter(session, id, params),
        "palette.setWildcard" => cmd_palette_set_wildcard(session, id, params),
        "palette.setColor" => cmd_palette_set_color(session, id, params),
        "palette.removeColor" => cmd_palette_remove_color(session, id, params),
        "palette.reset" => cmd_palette_reset(session, id, params),
        "palette.selectPreset" => cmd_palette_select_preset(session, id, params),
        "palette.savePreset" => cmd_palette_save_preset(session, id, params),
        "palette.deletePreset" => cmd_palette_delete_preset(session, id, params),
        "preset.save" => cmd_preset_save(session, id, params),
        "preset.applyCustom" => cmd_preset_apply_custom(session, id, params),
        "preset.delete" => cmd_preset_delete(session, id, params),
        "element.save" => cmd_element_save(session, id, params),
        "element.place" => cmd_element_place(session, id, params),
        "element.delete" => cmd_element_delete(session, id, params),
        "styleFavorite.save" => cmd_style_favorite_save(session, id, params),
        "styleFavorite.apply" => cmd_style_favorite_apply(session, id, params),
        "styleFavorite.delete" => cmd_style_favorite_delete(session, id, params),
        "theme.set" => cmd_theme_set(session, id, params),
        "stylus.setButtons" => cmd_stylus_set_buttons(session, id, params),
        "sync.status" => cmd_sync_status(session),
        "sync.setServerUrl" => cmd_sync_set_server_url(session, id, params),
        "sync.signOut" => cmd_sync_sign_out(session),
        "sync.transferNotebook" => cmd_sync_transfer_notebook(session, id, params),
        "sync.migrateLocalAccount" => cmd_sync_migrate_local_account(session, id, params),
        "profile.completeFirstAccess" => cmd_profile_complete_first_access(session, id, params),
        "profile.create" => cmd_profile_create(session, id, params),
        "profile.rename" => cmd_profile_rename(session, id, params),
        "profile.switch" => cmd_profile_switch(session, id, params),
        "profile.delete" => cmd_profile_delete(session, id, params),
        "studyTape.add" => crate::study::cmd_tape_add(session, id, params),
        "studyTape.reveal" => crate::study::cmd_tape_reveal(session, id, params),
        "studyTape.remove" => crate::study::cmd_tape_remove(session, id, params),
        "flashcard.add" => crate::study::cmd_card_add(session, id, params),
        "flashcard.remove" => crate::study::cmd_card_remove(session, id, params),
        "flashcard.review" => crate::study::cmd_card_review(session, id, params),
        "flashcard.due" => crate::study::cmd_card_due(session, id, params),
        "prefs.setStudy" => crate::study::cmd_study_prefs(session, id, params),
        _ => return Err(EngineError::UnknownCommand(id.to_string())),
    };
    if result.is_ok() {
        if id.starts_with("palette.")
            || id.starts_with("preset.")
            || id == "element.save"
            || id == "element.delete"
            || id == "styleFavorite.save"
            || id == "styleFavorite.delete"
            || id == "theme.set"
            || id == "prefs.setStudy"
            || id == "stylus.setButtons"
            || id.starts_with("library.")
            || id.starts_with("template.")
        {
            session.sync_mark_prefs_dirty();
        }
        if session.revision != prev_rev
            && !matches!(
                id,
                "page.select"
                    | "notebook.select"
                    | "theme.set"
                    | "stylus.setButtons"
                    | "library.setViewPrefs"
                    | "library.folder.setColor"
                    | "library.folder.setFavorite"
                    | "library.tag.setColor"
                    | "styleFavorite.apply"
                    | "profile.create"
                    | "profile.completeFirstAccess"
                    | "profile.rename"
                    | "profile.switch"
                    | "profile.delete"
                    | "sync.signOut"
                    | "sync.transferNotebook"
                    | "sync.migrateLocalAccount"
                    | "package.export"
                    | "package.import"
            )
        {
            session.bump_active_doc_modified_timestamp();
            session.mark_offline_edits_if_disconnected();
        }
        session.sync_active_notebook_slot();
    }
    result
}

fn cmd_stroke_draw(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: DrawParams = parse_params(id, params)?;
    if p.points.is_empty() {
        return Err(invalid(id, "points must not be empty"));
    }
    if p.points.len() > MAX_COMMAND_POINTS {
        return Err(invalid(id, "points exceed maximum stroke sample cap"));
    }
    if !p.points.iter().all(|s| s.x.is_finite() && s.y.is_finite()) {
        return Err(invalid(id, "stroke points contain non-finite coordinates"));
    }
    let pen = match p.pen {
        Some(custom) => custom.sanitized(),
        None => {
            let kind = p.kind.unwrap_or_else(|| session.active_pen_kind());
            session.pen(kind).sanitized()
        }
    };
    let clean_raw: Vec<StrokeSample> = p.points.into_iter().map(StrokeSample::sanitized).collect();
    let stroke_id = session
        .commit_raw_stroke(pen, clean_raw)
        .ok_or_else(|| invalid(id, "failed to build valid stroke"))?;
    let point_count = session
        .doc
        .strokes
        .last()
        .map_or(0, |s| s.smoothed_points.len());
    Ok(json!({ "strokeId": stroke_id.0, "pointCount": point_count }))
}

fn cmd_text_insert(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: InsertTextParams = parse_params(id, params)?;
    if !p.x.is_finite() || !p.y.is_finite() {
        return Err(invalid(id, "text position must contain finite coordinates"));
    }
    if p.size.is_some_and(|size| !size.is_finite()) {
        return Err(invalid(id, "text size must be finite"));
    }
    if p.line_spacing.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "text line spacing must be finite"));
    }
    if p.letter_spacing.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "text letter spacing must be finite"));
    }
    if p.max_width.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "text max width must be finite"));
    }
    if p.angle_degrees.is_some_and(|a| !a.is_finite()) {
        return Err(invalid(id, "text angle must be finite"));
    }
    let link_url = match p.link_url.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            Some(sanitize_link_url(raw).ok_or_else(|| invalid(id, "invalid link URL"))?)
        }
        _ => None,
    };
    if p.x < 0.0
        || p.y < 0.0
        || p.x > f64::from(session.doc.width)
        || p.y > f64::from(session.doc.height)
    {
        return Err(invalid(id, "text position must be inside the active page"));
    }
    if p.text.chars().count() > MAX_TEXT_CHARS {
        return Err(invalid(id, "text exceeds the maximum character limit"));
    }
    let item = TextItem {
        text: p.text,
        x: p.x,
        y: p.y,
        size: p.size.unwrap_or(24.0),
        angle_degrees: p.angle_degrees.unwrap_or(0.0),
        color: p.color.unwrap_or_else(|| session.active_pen().color),
        font: p.font.unwrap_or_default(),
        line_spacing: p.line_spacing.unwrap_or(DEFAULT_TEXT_LINE_SPACING),
        letter_spacing: p.letter_spacing.unwrap_or(DEFAULT_TEXT_LETTER_SPACING),
        max_width: p.max_width,
        link_url,
        alignment: p.alignment.unwrap_or_default(),
        spans: p.spans.unwrap_or_default(),
    }
    .sanitized();
    if item.text.trim().is_empty() {
        return Err(invalid(id, "text must not be empty"));
    }
    if session.doc.texts.len() >= MAX_PAGE_TEXT_ITEMS {
        return Err(invalid(id, "active page text item limit reached"));
    }
    let current_chars = session.doc.texts.iter().fold(0_usize, |total, current| {
        total.saturating_add(current.text.chars().count())
    });
    if current_chars.saturating_add(item.text.chars().count()) > MAX_PAGE_TEXT_CHARS {
        return Err(invalid(id, "active page text character limit reached"));
    }

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    next.texts.push(Arc::new(item));
    next.sync_active_to_pages();
    let text_count = next.texts.len();
    session.history.record("Insert Text", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({
        "textCount": text_count,
        "textIndex": text_count.saturating_sub(1),
    }))
}

fn cmd_text_update(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: UpdateTextParams = parse_params(id, params)?;
    let Some(current) = session.doc.texts.get(p.index) else {
        return Err(invalid(id, "text item does not exist on the active page"));
    };
    if p.x.is_some_and(|v| !v.is_finite()) || p.y.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "text position must contain finite coordinates"));
    }
    if p.size.is_some_and(|size| !size.is_finite()) {
        return Err(invalid(id, "text size must be finite"));
    }
    if p.line_spacing.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "text line spacing must be finite"));
    }
    if p.letter_spacing.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "text letter spacing must be finite"));
    }
    if p.max_width.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "text max width must be finite"));
    }
    if let Some(text) = &p.text {
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(invalid(id, "text exceeds the maximum character limit"));
        }
        if text.trim().is_empty() {
            return Err(invalid(id, "text must not be empty"));
        }
        let other_chars = session
            .doc
            .texts
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != p.index)
            .fold(0_usize, |total, (_, item)| {
                total.saturating_add(item.text.chars().count())
            });
        if other_chars.saturating_add(text.chars().count()) > MAX_PAGE_TEXT_CHARS {
            return Err(invalid(id, "active page text character limit reached"));
        }
    }

    let mut item = current.as_ref().clone();
    if let Some(text) = p.text {
        item.text = text;
    }
    if let Some(x) = p.x {
        item.x = x;
    }
    if let Some(y) = p.y {
        item.y = y;
    }
    if let Some(size) = p.size {
        item.size = size;
    }
    if let Some(color) = p.color {
        item.color = color;
    }
    if let Some(font) = p.font {
        item.font = font;
    }
    if let Some(line_spacing) = p.line_spacing {
        item.line_spacing = line_spacing;
    }
    if let Some(letter_spacing) = p.letter_spacing {
        item.letter_spacing = letter_spacing;
    }
    if p.clear_max_width.unwrap_or(false) {
        item.max_width = None;
    } else if let Some(max_width) = p.max_width {
        item.max_width = Some(max_width);
    }
    if p.clear_link_url.unwrap_or(false) {
        item.link_url = None;
    } else if let Some(raw_link) = &p.link_url {
        if raw_link.trim().is_empty() {
            item.link_url = None;
        } else {
            item.link_url =
                Some(sanitize_link_url(raw_link).ok_or_else(|| invalid(id, "invalid link URL"))?);
        }
    }
    if let Some(alignment) = p.alignment {
        item.alignment = alignment;
    }
    if let Some(spans) = p.spans {
        item.spans = spans;
    } else {
        let full_len = item.text.len();
        if p.size.is_some() {
            item.set_span_size(0, full_len, None);
        }
        if p.color.is_some() {
            item.set_span_color(0, full_len, None);
        }
        if p.font.is_some() {
            item.set_span_font(0, full_len, None);
        }
    }
    item = item.sanitized();

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(target) = next.texts.get_mut(p.index) else {
        return Err(invalid(id, "text item does not exist on the active page"));
    };
    *target = Arc::new(item);
    next.sync_active_to_pages();
    if p.record_history.unwrap_or(true) {
        session.history.record("Edit Text", before);
    }
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "textIndex": p.index }))
}

fn cmd_stroke_update(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: UpdateStrokeParams = parse_params(id, params)?;
    if p.size.is_some_and(|size| !size.is_finite()) {
        return Err(invalid(id, "stroke width must be finite"));
    }
    let Some(current) = session.doc.strokes.iter().find(|stroke| stroke.id == p.id) else {
        return Err(invalid(id, "stroke does not exist on the active page"));
    };

    let mut stroke = current.as_ref().clone();
    if let Some(size) = p.size {
        stroke.pen.size = size;
    }
    if let Some(color) = p.color {
        stroke.pen.color = color;
    }
    stroke.pen = stroke.pen.sanitized();
    stroke.bounds = drawcraft_doc::compute_stroke_bounds(&stroke.smoothed_points, stroke.pen.size);

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(target) = next.strokes.iter_mut().find(|item| item.id == p.id) else {
        return Err(invalid(id, "stroke does not exist on the active page"));
    };
    *target = Arc::new(stroke);
    next.sync_active_to_pages();
    if p.record_history.unwrap_or(true) {
        session.history.record("Edit Stroke", before);
    }
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "strokeId": p.id.0 }))
}

fn cmd_shape_insert(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: InsertShapeParams = parse_params(id, params)?;
    if !(p.x1.is_finite() && p.y1.is_finite() && p.x2.is_finite() && p.y2.is_finite()) {
        return Err(invalid(id, "shape coordinates must be finite"));
    }
    let max_x = f64::from(session.doc.width);
    let max_y = f64::from(session.doc.height);
    if [p.x1, p.x2].iter().any(|x| *x < 0.0 || *x > max_x)
        || [p.y1, p.y2].iter().any(|y| *y < 0.0 || *y > max_y)
    {
        return Err(invalid(id, "shape must stay inside the active page"));
    }
    if (p.x2 - p.x1).hypot(p.y2 - p.y1) < 1.0 {
        return Err(invalid(id, "shape must be at least one page point long"));
    }
    if session.doc.shapes.len() >= MAX_PAGE_SHAPES {
        return Err(invalid(id, "active page shape limit reached"));
    }
    if p.angle_degrees.is_some_and(|value| !value.is_finite())
        || p.curve.is_some_and(|value| !value.is_finite())
        || p.width.is_some_and(|value| !value.is_finite())
    {
        return Err(invalid(id, "shape style numbers must be finite"));
    }
    let points = if let Some(pts) = p.points {
        if pts.len() > MAX_POLYGON_VERTICES {
            return Err(invalid(id, "polygon vertex count exceeds limit"));
        }
        if pts.iter().any(|pt| !pt.x.is_finite() || !pt.y.is_finite()) {
            return Err(invalid(
                id,
                "polygon points must contain finite coordinates",
            ));
        }
        pts
    } else {
        Vec::new()
    };
    let shape = GeometricShape {
        kind: p.kind,
        x1: p.x1,
        y1: p.y1,
        x2: p.x2,
        y2: p.y2,
        angle_degrees: p.angle_degrees.unwrap_or(0.0),
        curve: p.curve.unwrap_or(0.0),
        width: p.width.unwrap_or_else(|| session.active_pen().size),
        color: p.color.unwrap_or_else(|| session.active_pen().color),
        fill_color: p.fill_color,
        line_style: p.line_style.unwrap_or_default(),
        points,
    }
    .sanitized();

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    next.shapes.push(Arc::new(shape));
    next.sync_active_to_pages();
    let shape_count = next.shapes.len();
    session.history.record("Insert Shape", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "shapeCount": shape_count }))
}

fn cmd_shape_update(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: UpdateShapeParams = parse_params(id, params)?;
    if p.angle_degrees.is_some_and(|value| !value.is_finite()) {
        return Err(invalid(id, "shape angle must be finite"));
    }
    if p.curve.is_some_and(|value| !value.is_finite()) {
        return Err(invalid(id, "shape curve must be finite"));
    }
    if p.width.is_some_and(|value| !value.is_finite()) {
        return Err(invalid(id, "shape width must be finite"));
    }
    let Some(current) = session.doc.shapes.get(p.index) else {
        return Err(invalid(id, "shape does not exist on the active page"));
    };

    let mut shape = current.as_ref().clone();
    if let Some(kind) = p.kind {
        shape.kind = kind;
    }
    if let Some(angle_degrees) = p.angle_degrees {
        shape.angle_degrees = angle_degrees;
    }
    if let Some(curve) = p.curve {
        shape.curve = curve;
    }
    if let Some(width) = p.width {
        shape.width = width;
    }
    if let Some(color) = p.color {
        shape.color = color;
    }
    if p.clear_fill.unwrap_or(false) {
        shape.fill_color = None;
    } else if let Some(fill) = p.fill_color {
        shape.fill_color = Some(fill);
    }
    if let Some(line_style) = p.line_style {
        shape.line_style = line_style;
    }
    shape = shape.sanitized();

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(target) = next.shapes.get_mut(p.index) else {
        return Err(invalid(id, "shape does not exist on the active page"));
    };
    *target = Arc::new(shape);
    next.sync_active_to_pages();
    if p.record_history.unwrap_or(true) {
        session.history.record("Edit Shape", before);
    }
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "shapeIndex": p.index }))
}

const MAX_RAW_IMAGE_BYTES: usize = 64 * 1024 * 1024;

fn looks_like_tga_header(raw_bytes: &[u8]) -> bool {
    if raw_bytes.len() < 18 {
        return false;
    }
    if raw_bytes.len() >= 26
        && &raw_bytes[raw_bytes.len() - 18..raw_bytes.len() - 2] == b"TRUEVISION-XFILE"
    {
        return true;
    }
    let id_len = usize::from(raw_bytes[0]);
    let color_map_type = raw_bytes[1];
    let image_type = raw_bytes[2];
    let valid_type = matches!(image_type, 1 | 2 | 3 | 9 | 10 | 11);
    let width = u16::from_le_bytes([raw_bytes[12], raw_bytes[13]]);
    let height = u16::from_le_bytes([raw_bytes[14], raw_bytes[15]]);
    let depth = raw_bytes[16];
    let descriptor = raw_bytes[17];
    let color_map_ok = match color_map_type {
        0 => raw_bytes[3..8] == [0, 0, 0, 0, 0],
        1 => raw_bytes[5..7] != [0, 0],
        _ => false,
    };
    valid_type
        && color_map_ok
        && width > 0
        && height > 0
        && matches!(depth, 8 | 15 | 16 | 24 | 32)
        && (descriptor & 0xC0) == 0
        && raw_bytes.len() > 18 + id_len
}

fn decode_image_with_orientation(
    id: &str,
    raw_bytes: &[u8],
) -> Result<(Option<image::ImageFormat>, image::DynamicImage), EngineError> {
    let reader = image::ImageReader::new(std::io::Cursor::new(raw_bytes))
        .with_guessed_format()
        .map_err(|e| invalid(id, format!("failed reading image bytes: {e}")))?;
    let format = reader.format();
    if format.is_some() {
        let mut decoder = reader
            .into_decoder()
            .map_err(|e| invalid(id, format!("unsupported or corrupted image: {e}")))?;
        let orientation = decoder
            .orientation()
            .unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut decoded = image::DynamicImage::from_decoder(decoder)
            .map_err(|e| invalid(id, format!("unsupported or corrupted image: {e}")))?;
        decoded.apply_orientation(orientation);
        return Ok((format, decoded));
    }
    if looks_like_tga_header(raw_bytes) {
        let mut tga_reader = image::ImageReader::new(std::io::Cursor::new(raw_bytes));
        tga_reader.set_format(image::ImageFormat::Tga);
        let decoded = tga_reader
            .decode()
            .map_err(|e| invalid(id, format!("unsupported or corrupted image: {e}")))?;
        return Ok((Some(image::ImageFormat::Tga), decoded));
    }
    Err(invalid(id, "unsupported or unrecognized image format"))
}

fn decode_and_normalize_image(
    id: &str,
    raw_bytes: &[u8],
) -> Result<(u32, u32, Vec<u8>), EngineError> {
    const MAX_NORMALIZED_DIM: u32 = 2048;

    if raw_bytes.is_empty() {
        return Err(invalid(id, "image payload must not be empty"));
    }
    if raw_bytes.len() > MAX_RAW_IMAGE_BYTES {
        return Err(invalid(id, "raw image payload exceeds 64 MiB limit"));
    }
    let (format, decoded) = decode_image_with_orientation(id, raw_bytes)?;
    let mut rgba = decoded.to_rgba8();
    if matches!(format, Some(image::ImageFormat::Bmp)) && rgba.pixels().all(|pixel| pixel.0[3] == 0)
    {
        for pixel in rgba.pixels_mut() {
            pixel.0[3] = 255;
        }
    }
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 {
        return Err(invalid(id, "image dimensions must be non-zero"));
    }
    let mut rgba = if w > MAX_NORMALIZED_DIM || h > MAX_NORMALIZED_DIM {
        let scale = (f64::from(MAX_NORMALIZED_DIM) / f64::from(w))
            .min(f64::from(MAX_NORMALIZED_DIM) / f64::from(h));
        let new_w = (f64::from(w) * scale)
            .round()
            .clamp(1.0, f64::from(MAX_NORMALIZED_DIM)) as u32;
        let new_h = (f64::from(h) * scale)
            .round()
            .clamp(1.0, f64::from(MAX_NORMALIZED_DIM)) as u32;
        image::imageops::resize(&rgba, new_w, new_h, image::imageops::FilterType::Triangle)
    } else {
        rgba
    };

    let mut png = drawcraft_raster::png_encode(rgba.width(), rgba.height(), rgba.as_raw())
        .map_err(|e| invalid(id, format!("failed to encode PNG: {e}")))?;
    for _ in 0..4 {
        if png.len() <= MAX_IMAGE_PNG_BYTES {
            break;
        }
        let next_w = ((rgba.width() * 3) / 4).max(256).min(rgba.width());
        let next_h = ((rgba.height() * 3) / 4).max(256).min(rgba.height());
        if next_w == rgba.width() && next_h == rgba.height() {
            break;
        }
        rgba =
            image::imageops::resize(&rgba, next_w, next_h, image::imageops::FilterType::Triangle);
        png = drawcraft_raster::png_encode(rgba.width(), rgba.height(), rgba.as_raw())
            .map_err(|e| invalid(id, format!("failed to encode PNG: {e}")))?;
    }
    if png.len() > MAX_IMAGE_PNG_BYTES {
        return Err(invalid(
            id,
            "encoded image exceeds maximum per-image payload limit",
        ));
    }
    Ok((rgba.width(), rgba.height(), png))
}

fn extract_image_raw_bytes(id: &str, p: &InsertImageParams) -> Result<Vec<u8>, EngineError> {
    if let Some(bytes) = &p.bytes {
        return Ok(bytes.clone());
    }
    if let Some(b64) = &p.base64_bytes {
        return base64_decode_bytes_with_limit(b64, MAX_RAW_IMAGE_BYTES)
            .map_err(|reason| invalid(id, reason));
    }
    Err(invalid(id, "provide either `bytes` or `base64_bytes`"))
}

fn resolve_image_placement(
    doc: &Document,
    px_w: u32,
    px_h: u32,
    p: &InsertImageParams,
    id: &str,
) -> Result<(f64, f64, f64, f64), EngineError> {
    for opt in [p.x, p.y, p.width, p.height, p.angle_degrees] {
        if opt.is_some_and(|value| !value.is_finite()) {
            return Err(invalid(id, "image geometry values must be finite"));
        }
    }
    let page_w = f64::from(doc.width.max(64));
    let page_h = f64::from(doc.height.max(64));
    let nat_w = f64::from(px_w.max(1));
    let nat_h = f64::from(px_h.max(1));
    let (width, height) = match (p.width, p.height) {
        (Some(w), Some(h)) => (w.clamp(8.0, 8192.0), h.clamp(8.0, 8192.0)),
        (Some(w), None) => {
            let w = w.clamp(8.0, 8192.0);
            let h = (w * (nat_h / nat_w)).clamp(8.0, 8192.0);
            (w, h)
        }
        (None, Some(h)) => {
            let h = h.clamp(8.0, 8192.0);
            let w = (h * (nat_w / nat_h)).clamp(8.0, 8192.0);
            (w, h)
        }
        (None, None) => {
            let max_w = (page_w * 0.6).max(64.0);
            let max_h = (page_h * 0.6).max(64.0);
            let mut scale = (max_w / nat_w).min(max_h / nat_h).min(1.0);
            if nat_w < 24.0 && nat_h < 24.0 {
                scale = (24.0 / nat_w)
                    .min(24.0 / nat_h)
                    .min(max_w / nat_w)
                    .min(max_h / nat_h);
            }
            let w = (nat_w * scale).clamp(8.0, 8192.0);
            let h = (nat_h * scale).clamp(8.0, 8192.0);
            (w, h)
        }
    };
    let max_x = (page_w - width).max(0.0);
    let max_y = (page_h - height).max(0.0);
    let x = p.x.unwrap_or((page_w - width) * 0.5).clamp(0.0, max_x);
    let y = p.y.unwrap_or((page_h - height) * 0.5).clamp(0.0, max_y);
    Ok((x, y, width, height))
}

fn cmd_image_insert(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: InsertImageParams = parse_params(id, params)?;
    if session.doc.images.len() >= MAX_PAGE_IMAGES {
        return Err(invalid(id, "active page image limit reached"));
    }
    let raw_bytes = extract_image_raw_bytes(id, &p)?;
    let (px_w, px_h, png_bytes) = decode_and_normalize_image(id, &raw_bytes)?;
    let used_bytes: usize = session
        .doc
        .images
        .iter()
        .map(|img| img.png_bytes.len())
        .sum();
    if used_bytes.saturating_add(png_bytes.len()) > MAX_PAGE_IMAGE_BYTES {
        return Err(invalid(id, "active page total image payload limit reached"));
    }
    let (x, y, width, height) = resolve_image_placement(&session.doc, px_w, px_h, &p, id)?;
    let image = PageImage {
        x,
        y,
        width,
        height,
        angle_degrees: p.angle_degrees.unwrap_or(0.0),
        pixel_width: px_w,
        pixel_height: px_h,
        png_bytes: Arc::new(png_bytes),
    }
    .sanitized();

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    next.images.push(Arc::new(image));
    next.sync_active_to_pages();
    let image_count = next.images.len();
    let image_index = image_count.saturating_sub(1);
    session.history.record("Insert Image", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "imageIndex": image_index, "imageCount": image_count }))
}

fn cmd_image_update(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: UpdateImageParams = parse_params(id, params)?;
    for opt in [p.x, p.y, p.width, p.height, p.angle_degrees] {
        if opt.is_some_and(|value| !value.is_finite()) {
            return Err(invalid(id, "image geometry values must be finite"));
        }
    }
    let Some(current) = session.doc.images.get(p.index) else {
        return Err(invalid(id, "image does not exist on the active page"));
    };
    let mut image = current.as_ref().clone();
    if let Some(x) = p.x {
        image.x = x;
    }
    if let Some(y) = p.y {
        image.y = y;
    }
    if let Some(width) = p.width {
        image.width = width;
    }
    if let Some(height) = p.height {
        image.height = height;
    }
    if let Some(angle) = p.angle_degrees {
        image.angle_degrees = angle;
    }
    image = image.sanitized();

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(target) = next.images.get_mut(p.index) else {
        return Err(invalid(id, "image does not exist on the active page"));
    };
    *target = Arc::new(image);
    next.sync_active_to_pages();
    session.history.record("Edit Image", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "imageIndex": p.index }))
}

fn transform_point(
    point: Point,
    pivot: Point,
    translation: Point,
    scale_x: f64,
    scale_y: f64,
    angle_radians: f64,
) -> Point {
    let dx = (point.x - pivot.x) * scale_x;
    let dy = (point.y - pivot.y) * scale_y;
    let (sin, cos) = angle_radians.sin_cos();
    Point::new(
        pivot.x + dx * cos - dy * sin + translation.x,
        pivot.y + dx * sin + dy * cos + translation.y,
    )
}

fn cmd_transform_selection(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: TransformSelectionParams = parse_params(id, params)?;
    if p.targets.is_empty() || p.targets.len() > 8192 {
        return Err(invalid(
            id,
            "selection must contain between 1 and 8192 objects",
        ));
    }
    if !p.pivot.is_finite()
        || !p.translation.is_finite()
        || p.pivot.x.abs() > 1e6
        || p.pivot.y.abs() > 1e6
        || p.translation.x.abs() > 1e6
        || p.translation.y.abs() > 1e6
    {
        return Err(invalid(
            id,
            "transform coordinates must be finite and bounded",
        ));
    }
    if !p.scale.is_finite() || !(0.01..=100.0).contains(&p.scale) {
        return Err(invalid(id, "transform scale must be between 0.01 and 100"));
    }
    let scale_x = p.scale_x.unwrap_or(p.scale);
    let scale_y = p.scale_y.unwrap_or(p.scale);
    if !scale_x.is_finite()
        || !(0.01..=100.0).contains(&scale_x)
        || !scale_y.is_finite()
        || !(0.01..=100.0).contains(&scale_y)
    {
        return Err(invalid(
            id,
            "transform axis scales must be between 0.01 and 100",
        ));
    }
    if !p.rotation_degrees.is_finite() {
        return Err(invalid(id, "transform rotation must be finite"));
    }
    let stroke_indexes: HashMap<_, _> = session
        .doc
        .strokes
        .iter()
        .enumerate()
        .map(|(index, stroke)| (stroke.id, index))
        .collect();
    let mut seen_targets = HashSet::with_capacity(p.targets.len());
    for target in &p.targets {
        if !seen_targets.insert(*target) {
            return Err(invalid(id, "selection contains a duplicate object"));
        }
        let exists = match target {
            TransformTarget::Stroke { id } => stroke_indexes.contains_key(id),
            TransformTarget::Shape { index } => *index < session.doc.shapes.len(),
            TransformTarget::Text { index } => *index < session.doc.texts.len(),
            TransformTarget::Image { index } => *index < session.doc.images.len(),
        };
        if !exists {
            return Err(invalid(
                id,
                "a selected object no longer exists on the active page",
            ));
        }
    }

    let pivot = p.pivot;
    let translation = p.translation;
    let is_uniform_scale = (scale_x - scale_y).abs() <= 1e-6;
    let rotation_degrees = p.rotation_degrees.rem_euclid(360.0);
    let angle_radians = rotation_degrees.to_radians();
    let object_count = p.targets.len();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    for target in p.targets {
        match target {
            TransformTarget::Stroke { id: stroke_id } => {
                let Some(stroke_index) = stroke_indexes.get(&stroke_id).copied() else {
                    return Err(invalid(id, "stroke does not exist on the active page"));
                };
                let Some(current) = next.strokes.get(stroke_index).cloned() else {
                    return Err(invalid(id, "stroke does not exist on the active page"));
                };
                let mut stroke = current.as_ref().clone();
                let transform_samples = |samples: &[StrokeSample]| {
                    samples
                        .iter()
                        .map(|sample| {
                            let point = transform_point(
                                sample.point(),
                                pivot,
                                translation,
                                scale_x,
                                scale_y,
                                angle_radians,
                            );
                            let (sin, cos) = (angle_radians as f32).sin_cos();
                            StrokeSample {
                                x: point.x,
                                y: point.y,
                                tilt_x: sample.tilt_x * cos - sample.tilt_y * sin,
                                tilt_y: sample.tilt_x * sin + sample.tilt_y * cos,
                                rotation: (sample.rotation + rotation_degrees as f32)
                                    .rem_euclid(360.0),
                                ..*sample
                            }
                            .sanitized()
                        })
                        .collect::<Vec<_>>()
                };
                stroke.raw_points = Arc::new(transform_samples(&stroke.raw_points));
                stroke.smoothed_points = Arc::new(transform_samples(&stroke.smoothed_points));
                stroke.pen.nib_angle_deg =
                    ((f64::from(stroke.pen.nib_angle_deg) + rotation_degrees + 180.0)
                        .rem_euclid(360.0)
                        - 180.0) as f32;
                stroke.pen = stroke.pen.sanitized();
                stroke.bounds =
                    drawcraft_doc::compute_stroke_bounds(&stroke.smoothed_points, stroke.pen.size);
                if let Some(target) = next.strokes.get_mut(stroke_index) {
                    *target = Arc::new(stroke);
                }
            }
            TransformTarget::Shape { index } => {
                let Some(current) = next.shapes.get(index).cloned() else {
                    return Err(invalid(id, "shape does not exist on the active page"));
                };
                let mut shape = current.as_ref().clone();
                let center = Point::new((shape.x1 + shape.x2) * 0.5, (shape.y1 + shape.y2) * 0.5);
                let transformed_center =
                    transform_point(center, pivot, translation, scale_x, scale_y, angle_radians);
                let (sin, cos) = shape.angle_degrees.to_radians().sin_cos();
                let local_scale_x = (scale_x * cos).hypot(scale_y * sin);
                let local_scale_y = (scale_x * sin).hypot(scale_y * cos);
                let half_x = (shape.x2 - shape.x1) * local_scale_x * 0.5;
                let half_y = (shape.y2 - shape.y1) * local_scale_y * 0.5;
                shape.x1 = transformed_center.x - half_x;
                shape.y1 = transformed_center.y - half_y;
                shape.x2 = transformed_center.x + half_x;
                shape.y2 = transformed_center.y + half_y;
                for pt in &mut shape.points {
                    let dx = pt.x - center.x;
                    let dy = pt.y - center.y;
                    let lx = dx * cos + dy * sin;
                    let ly = -dx * sin + dy * cos;
                    let slx = lx * local_scale_x;
                    let sly = ly * local_scale_y;
                    pt.x = transformed_center.x + slx * cos - sly * sin;
                    pt.y = transformed_center.y + slx * sin + sly * cos;
                }
                shape.angle_degrees += rotation_degrees;
                if let Some(target) = next.shapes.get_mut(index) {
                    *target = Arc::new(shape.sanitized());
                }
            }
            TransformTarget::Text { index } => {
                let Some(current) = next.texts.get(index).cloned() else {
                    return Err(invalid(id, "text item does not exist on the active page"));
                };
                let mut item = current.as_ref().clone();
                let anchor = transform_point(
                    Point::new(item.x, item.y),
                    pivot,
                    translation,
                    scale_x,
                    scale_y,
                    angle_radians,
                );
                item.x = anchor.x;
                item.y = anchor.y;
                if is_uniform_scale {
                    let old_size = f64::from(item.size);
                    let new_size = (old_size * scale_x).clamp(6.0, 120.0);
                    let size_scale = if old_size > f64::EPSILON {
                        new_size / old_size
                    } else {
                        scale_x
                    };
                    item.size = new_size as f32;
                    item.max_width = item
                        .max_width
                        .map(|width| (f64::from(width) * size_scale) as f32);
                } else if (scale_x - 1.0).abs() > 1e-6 {
                    item.max_width = item
                        .max_width
                        .map(|width| (f64::from(width) * scale_x) as f32);
                }
                item.angle_degrees += rotation_degrees;
                if let Some(target) = next.texts.get_mut(index) {
                    *target = Arc::new(item.sanitized());
                }
            }
            TransformTarget::Image { index } => {
                let Some(current) = next.images.get(index).cloned() else {
                    return Err(invalid(id, "image does not exist on the active page"));
                };
                let mut image = current.as_ref().clone();
                let transformed_center = transform_point(
                    image.center(),
                    pivot,
                    translation,
                    scale_x,
                    scale_y,
                    angle_radians,
                );
                let (sin, cos) = image.angle_degrees.to_radians().sin_cos();
                let local_scale_x = (scale_x * cos).hypot(scale_y * sin);
                let local_scale_y = (scale_x * sin).hypot(scale_y * cos);
                image.width = (image.width * local_scale_x).clamp(8.0, 8192.0);
                image.height = (image.height * local_scale_y).clamp(8.0, 8192.0);
                image.x = transformed_center.x - image.width * 0.5;
                image.y = transformed_center.y - image.height * 0.5;
                image.angle_degrees += rotation_degrees;
                if let Some(target) = next.images.get_mut(index) {
                    *target = Arc::new(image.sanitized());
                }
            }
        }
    }
    next.sync_active_to_pages();
    if p.record_history.unwrap_or(true) {
        session.history.record("Transform Selection", before);
    }
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "objectCount": object_count }))
}

fn cmd_update_selection(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: UpdateSelectionParams = parse_params(id, params)?;
    if p.targets.is_empty() || p.targets.len() > 8192 {
        return Err(invalid(
            id,
            "selection must contain between 1 and 8192 objects",
        ));
    }
    if p.color.is_none() && p.shape_kind.is_none() {
        return Err(invalid(
            id,
            "provide a shared color or shape format to apply",
        ));
    }

    let stroke_indexes: HashMap<_, _> = session
        .doc
        .strokes
        .iter()
        .enumerate()
        .map(|(index, stroke)| (stroke.id, index))
        .collect();
    let mut seen_targets = HashSet::with_capacity(p.targets.len());
    for target in &p.targets {
        if !seen_targets.insert(*target) {
            return Err(invalid(id, "selection contains a duplicate object"));
        }
        let exists = match target {
            TransformTarget::Stroke { id } => stroke_indexes.contains_key(id),
            TransformTarget::Shape { index } => *index < session.doc.shapes.len(),
            TransformTarget::Text { index } => *index < session.doc.texts.len(),
            TransformTarget::Image { index } => *index < session.doc.images.len(),
        };
        if !exists {
            return Err(invalid(
                id,
                "a selected object no longer exists on the active page",
            ));
        }
        if p.shape_kind.is_some() && !matches!(target, TransformTarget::Shape { .. }) {
            return Err(invalid(
                id,
                "shape format can only be applied to geometric shapes",
            ));
        }
    }

    let object_count = p.targets.len();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    for target in p.targets {
        match target {
            TransformTarget::Stroke { id: stroke_id } => {
                let Some(stroke_index) = stroke_indexes.get(&stroke_id).copied() else {
                    return Err(invalid(id, "stroke does not exist on the active page"));
                };
                let Some(current) = next.strokes.get(stroke_index).cloned() else {
                    return Err(invalid(id, "stroke does not exist on the active page"));
                };
                let mut stroke = current.as_ref().clone();
                if let Some(color) = p.color {
                    stroke.pen.color = color;
                }
                stroke.pen = stroke.pen.sanitized();
                if let Some(target) = next.strokes.get_mut(stroke_index) {
                    *target = Arc::new(stroke);
                }
            }
            TransformTarget::Shape { index } => {
                let Some(current) = next.shapes.get(index).cloned() else {
                    return Err(invalid(id, "shape does not exist on the active page"));
                };
                let mut shape = current.as_ref().clone();
                if let Some(color) = p.color {
                    shape.color = color;
                }
                if let Some(kind) = p.shape_kind {
                    shape.kind = kind;
                }
                if p.clear_fill.unwrap_or(false) {
                    shape.fill_color = None;
                } else if let Some(fill) = p.fill_color {
                    shape.fill_color = Some(fill);
                }
                if let Some(line_style) = p.line_style {
                    shape.line_style = line_style;
                }
                if let Some(target) = next.shapes.get_mut(index) {
                    *target = Arc::new(shape.sanitized());
                }
            }
            TransformTarget::Text { index } => {
                let Some(current) = next.texts.get(index).cloned() else {
                    return Err(invalid(id, "text item does not exist on the active page"));
                };
                let mut item = current.as_ref().clone();
                if let Some(color) = p.color {
                    item.color = color;
                    let full_len = item.text.len();
                    item.set_span_color(0, full_len, None);
                }
                if let Some(target) = next.texts.get_mut(index) {
                    *target = Arc::new(item.sanitized());
                }
            }
            TransformTarget::Image { .. } => {}
        }
    }

    next.sync_active_to_pages();
    session.history.record("Edit Selection", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "objectCount": object_count }))
}

fn cmd_delete_selection(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DeleteSelectionParams = parse_params(id, params)?;
    if p.targets.is_empty() || p.targets.len() > 8192 {
        return Err(invalid(
            id,
            "selection must contain between 1 and 8192 objects",
        ));
    }
    let stroke_indexes: HashMap<_, _> = session
        .doc
        .strokes
        .iter()
        .enumerate()
        .map(|(index, stroke)| (stroke.id, index))
        .collect();
    let mut seen_targets = HashSet::with_capacity(p.targets.len());
    let mut stroke_ids = HashSet::new();
    let mut shape_indices = Vec::new();
    let mut text_indices = Vec::new();
    let mut image_indices = Vec::new();

    for target in p.targets {
        if !seen_targets.insert(target) {
            return Err(invalid(id, "selection contains a duplicate object"));
        }
        match target {
            TransformTarget::Stroke { id: sid } if stroke_indexes.contains_key(&sid) => {
                stroke_ids.insert(sid);
            }
            TransformTarget::Shape { index } if index < session.doc.shapes.len() => {
                shape_indices.push(index);
            }
            TransformTarget::Text { index } if index < session.doc.texts.len() => {
                text_indices.push(index);
            }
            TransformTarget::Image { index } if index < session.doc.images.len() => {
                image_indices.push(index);
            }
            _ => {
                return Err(invalid(
                    id,
                    "a selected object no longer exists on the active page",
                ));
            }
        }
    }

    shape_indices.sort_unstable_by(|a, b| b.cmp(a));
    text_indices.sort_unstable_by(|a, b| b.cmp(a));
    image_indices.sort_unstable_by(|a, b| b.cmp(a));

    let deleted_count = seen_targets.len();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    if !stroke_ids.is_empty() {
        next.strokes.retain(|s| !stroke_ids.contains(&s.id));
    }
    for index in shape_indices {
        next.shapes.remove(index);
    }
    for index in text_indices {
        next.texts.remove(index);
    }
    for index in image_indices {
        next.images.remove(index);
    }
    next.sync_active_to_pages();
    session.history.record("Delete Selection", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "deletedCount": deleted_count }))
}

fn cmd_stroke_erase(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: EraseParams = parse_params(id, params)?;
    if p.path.is_empty() {
        return Err(invalid(id, "path must not be empty"));
    }
    if p.path.len() > MAX_COMMAND_POINTS {
        return Err(invalid(id, "path exceeds maximum length"));
    }
    let default_r = p
        .radius
        .unwrap_or_else(|| f64::from(session.eraser_size * 0.5));
    if !default_r.is_finite() || default_r <= 0.0 {
        return Err(invalid(id, "radius must be finite and positive"));
    }
    let mut clean_path = Vec::with_capacity(p.path.len());
    for pt in p.path {
        if !(pt.x.is_finite() && pt.y.is_finite()) {
            return Err(invalid(id, "path contains non-finite coordinates"));
        }
        let r = pt.radius.unwrap_or(default_r);
        if !r.is_finite() || r <= 0.0 {
            return Err(invalid(id, "point radius must be finite and positive"));
        }
        clean_path.push((Point::new(pt.x, pt.y), r.clamp(1.0, 200.0)));
    }

    let mode = p.mode.unwrap_or(session.eraser_mode);
    let highlighter_only = p
        .highlighter_only
        .unwrap_or(session.eraser_highlighter_only);
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let (updated, changed) = erase_along_path_filtered(
        &next.strokes,
        &clean_path,
        mode,
        highlighter_only,
        &mut next.next_stroke_id,
    );
    if changed {
        next.strokes = updated;
        next.sync_active_to_pages();
        session.history.record("Erase", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({ "changed": changed, "strokeCount": session.doc.strokes.len() }))
}

fn cmd_undo(session: &mut Session) -> Result<Value, EngineError> {
    match session.history.undo(Arc::clone(&session.doc)) {
        Some((label, doc)) => {
            session.doc = doc;
            session.mark_active_dirty();
            session.bump_revision();
            Ok(json!({ "undone": true, "label": label }))
        }
        None => Ok(json!({ "undone": false, "label": Value::Null })),
    }
}

fn cmd_redo(session: &mut Session) -> Result<Value, EngineError> {
    match session.history.redo(Arc::clone(&session.doc)) {
        Some((label, doc)) => {
            session.doc = doc;
            session.mark_active_dirty();
            session.bump_revision();
            Ok(json!({ "redone": true, "label": label }))
        }
        None => Ok(json!({ "redone": false, "label": Value::Null })),
    }
}

fn cmd_page_clear(session: &mut Session) -> Result<Value, EngineError> {
    let count = session.doc.strokes.len();
    let text_count = session.doc.texts.len();
    let shape_count = session.doc.shapes.len();
    let image_count = session.doc.images.len();
    if count > 0 || text_count > 0 || shape_count > 0 || image_count > 0 {
        let before = Arc::clone(&session.doc);
        let mut next = (*session.doc).clone();
        next.strokes.clear();
        next.texts.clear();
        next.shapes.clear();
        next.images.clear();
        next.sync_active_to_pages();
        session.history.record("Clear Page", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({
        "cleared": count,
        "clearedText": text_count,
        "clearedShapes": shape_count,
        "clearedImages": image_count,
    }))
}

fn find_custom_template<'a>(
    prefs: &'a drawcraft_doc::UserStudioConfig,
    id_cmd: &str,
    raw_id: &str,
) -> Result<&'a CustomPaperTemplate, EngineError> {
    let clean_id = sanitize_template_id(raw_id);
    if clean_id.is_empty() {
        return Err(invalid(id_cmd, "custom template ID must not be empty"));
    }
    prefs
        .custom_templates
        .iter()
        .find(|t| t.id == clean_id)
        .ok_or_else(|| {
            invalid(
                id_cmd,
                format!("unknown custom paper template `{clean_id}`"),
            )
        })
}

fn validate_page_selection_ordered(
    id: &str,
    indices: &[usize],
    page_count: usize,
) -> Result<Vec<usize>, EngineError> {
    if indices.is_empty() {
        return Err(invalid(id, "select at least one page index"));
    }
    let mut seen = HashSet::new();
    let mut ordered = Vec::with_capacity(indices.len().min(page_count));
    for &idx in indices {
        if idx >= page_count {
            return Err(invalid(id, format!("page index {idx} out of bounds")));
        }
        if seen.insert(idx) {
            ordered.push(idx);
        }
    }
    Ok(ordered)
}

fn validate_page_selection(
    id: &str,
    indices: &[usize],
    page_count: usize,
) -> Result<Vec<usize>, EngineError> {
    let mut unique = validate_page_selection_ordered(id, indices, page_count)?;
    unique.sort_unstable();
    Ok(unique)
}

fn cmd_set_template(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SetTemplateParams = parse_params(id, params)?;
    if p.template.is_none()
        && p.color.is_none()
        && p.custom_template_id.is_none()
        && p.clear_custom_template.is_none()
    {
        return Err(invalid(
            id,
            "provide `template`, `color`, `custom_template_id`, and/or `clear_custom_template`",
        ));
    }
    let custom_tpl = match p.custom_template_id.as_deref() {
        Some(raw) => Some(find_custom_template(&session.prefs, id, raw)?.clone()),
        None => None,
    };
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let active_idx = next.active_page_index();
    if let Some(ref tpl) = custom_tpl {
        next.apply_custom_template_to_page(active_idx, tpl)
            .map_err(|reason| invalid(id, reason))?;
    } else if p.clear_custom_template == Some(true) {
        next.clear_page_custom_template(active_idx);
    }
    if let Some(t) = p.template {
        next.paper_template = t;
    }
    if let Some(c) = p.color {
        next.paper_color = c;
    }
    next.sync_active_to_pages();
    if next != *before {
        session.history.record("Page Template", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({
        "template": session.doc.paper_template,
        "color": session.doc.paper_color,
        "customTemplateId": session.doc.custom_template_id,
    }))
}

fn sanitize_io_path(id: &str, raw: &str) -> Result<PathBuf, EngineError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_PATH_LEN || trimmed.contains('\0') {
        return Err(invalid(
            id,
            "file path must be non-empty and under 512 characters",
        ));
    }
    if trimmed
        .chars()
        .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(invalid(id, "file path contains unsupported characters"));
    }
    let path = Path::new(trimmed);
    if path
        .components()
        .any(|comp| matches!(comp, Component::ParentDir))
    {
        return Err(invalid(
            id,
            "parent directory traversal (`..`) is not allowed",
        ));
    }
    let has_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|s| !s.is_empty());
    if !has_name {
        return Err(invalid(id, "file path must include a valid file name"));
    }
    Ok(path.to_path_buf())
}

fn write_export_bytes(
    id: &str,
    path_opt: Option<&str>,
    bytes: &[u8],
) -> Result<String, EngineError> {
    let raw =
        path_opt.ok_or_else(|| invalid(id, "provide output `path` or set `in_memory: true`"))?;
    let path = sanitize_io_path(id, raw)?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return Err(invalid(
            id,
            format!("failed to prepare export folder: {err}"),
        ));
    }
    if let Err(err) = std::fs::write(&path, bytes) {
        return Err(invalid(id, format!("failed to write export file: {err}")));
    }
    Ok(path.to_string_lossy().into_owned())
}

fn validate_page_dimension_bounds(
    id: &str,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(), EngineError> {
    if let Some(w) = width
        && !(64..=8192).contains(&w)
    {
        return Err(invalid(id, "page width must be between 64 and 8192"));
    }
    if let Some(h) = height
        && !(64..=8192).contains(&h)
    {
        return Err(invalid(id, "page height must be between 64 and 8192"));
    }
    Ok(())
}

fn resolve_requested_page_dimensions(
    fallback: (u32, u32),
    resolution: Option<PageResolutionPreset>,
    orientation: Option<PageOrientation>,
    width: Option<u32>,
    height: Option<u32>,
) -> (u32, u32) {
    let (mut w, mut h) = match resolution {
        Some(res) if res != PageResolutionPreset::Custom => res.dimensions(),
        _ => fallback,
    };
    if let Some(explicit_w) = width {
        w = explicit_w;
    }
    if let Some(explicit_h) = height {
        h = explicit_h;
    }
    if let Some(orient) = orientation {
        (w, h) = orient.apply(w, h);
    }
    (w.clamp(64, 8192), h.clamp(64, 8192))
}

fn cmd_page_add(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: AddPageParams = parse_optional_params(id, params)?;
    validate_page_dimension_bounds(id, p.width, p.height)?;
    let custom_tpl = match p.custom_template_id.as_deref() {
        Some(raw) => Some(find_custom_template(&session.prefs, id, raw)?.clone()),
        None => None,
    };
    let has_size_override = p.resolution.is_some()
        || p.orientation.is_some()
        || p.width.is_some()
        || p.height.is_some();
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let (req_w, req_h) = if has_size_override {
        let fallback = custom_tpl
            .as_ref()
            .map(|tpl| (tpl.width.clamp(64, 8192), tpl.height.clamp(64, 8192)))
            .unwrap_or((next.width, next.height));
        let (w, h) = resolve_requested_page_dimensions(
            fallback,
            p.resolution,
            p.orientation,
            p.width,
            p.height,
        );
        (Some(w), Some(h))
    } else {
        (None, None)
    };
    let Some(idx) = next.add_page_with_size(req_w, req_h, p.template, p.color) else {
        return Err(invalid(id, "notebook page limit reached"));
    };
    if let Some(ref tpl) = custom_tpl {
        next.apply_custom_template_to_page(idx, tpl)
            .map_err(|reason| invalid(id, reason))?;
        if let Some(c) = p.color {
            next.paper_color = c;
            next.sync_active_to_pages();
        }
        if let (Some(w), Some(h)) = (req_w, req_h) {
            next.set_page_dimensions(idx, w, h);
        }
    }
    session.history.record("Add Page", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({
        "pageIndex": idx,
        "pageCount": session.doc.page_count(),
        "width": session.doc.width,
        "height": session.doc.height,
    }))
}

fn cmd_page_select(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SelectIndexParams = parse_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let mut next = (*session.doc).clone();
    if !next.select_page(p.index) {
        return Err(invalid(id, "page index out of bounds"));
    }
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({
        "pageIndex": session.doc.active_page_index(),
        "pageCount": session.doc.page_count()
    }))
}

fn cmd_page_duplicate(session: &mut Session, id: &str) -> Result<Value, EngineError> {
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let Some(idx) = next.duplicate_active_page() else {
        return Err(invalid(id, "notebook page limit reached"));
    };
    session.history.record("Duplicate Page", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "pageIndex": idx, "pageCount": session.doc.page_count() }))
}

fn cmd_page_remove(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: OptionalIndexParams = parse_optional_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    if session.doc.page_count() <= 1 {
        return Err(invalid(id, "cannot remove the last remaining page"));
    }
    let target = p.index.unwrap_or_else(|| session.doc.active_page_index());
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    if !next.remove_page(target) {
        return Err(invalid(id, "page index out of bounds"));
    }
    session.history.record("Remove Page", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({
        "pageIndex": session.doc.active_page_index(),
        "pageCount": session.doc.page_count()
    }))
}

fn cmd_page_move(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: MovePageParams = parse_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let count = session.doc.page_count();
    if p.from >= count || p.to >= count {
        return Err(invalid(id, "page index out of bounds"));
    }
    if p.from != p.to {
        let before = Arc::clone(&session.doc);
        let mut next = (*session.doc).clone();
        if next.move_page(p.from, p.to) {
            session.history.record("Reorder Page", before);
            session.doc = Arc::new(next);
            session.mark_active_dirty();
            session.bump_revision();
        }
    }
    Ok(json!({
        "pageIndex": session.doc.active_page_index(),
        "pageCount": session.doc.page_count(),
    }))
}

fn cmd_page_set_bookmark(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: BookmarkPageParams = parse_optional_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    next.sync_active_to_pages();
    let target = p.index.unwrap_or_else(|| next.active_page_index());
    let Some(slot) = next.pages.get_mut(target) else {
        return Err(invalid(id, "page index out of bounds"));
    };
    let next_state = p.bookmarked.unwrap_or(!slot.bookmarked);
    if slot.bookmarked != next_state {
        slot.bookmarked = next_state;
        next.load_active_from_pages();
        session.history.record("Bookmark Page", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    let bookmarked = session
        .doc
        .pages
        .get(target)
        .is_some_and(|page| page.bookmarked);
    Ok(json!({ "pageIndex": target, "bookmarked": bookmarked }))
}

fn cmd_page_rotate(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: RotatePageParams = parse_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let turns = p.quarter_turns.rem_euclid(4);
    if turns == 0 {
        return Err(invalid(id, "quarter_turns must not be a multiple of 4"));
    }
    let page_count = session.doc.page_count();
    let targets = match p.pages {
        Some(ref list) => validate_page_selection(id, list, page_count)?,
        None => {
            let idx = p.index.unwrap_or_else(|| session.doc.active_page_index());
            if idx >= page_count {
                return Err(invalid(id, "page index out of bounds"));
            }
            vec![idx]
        }
    };
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let mut rotated_count = 0usize;
    for idx in &targets {
        if next.rotate_page_quarter_turns(*idx, turns) {
            rotated_count += 1;
        }
    }
    if rotated_count > 0 {
        session.history.record("Rotate Page", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({
        "rotatedCount": rotated_count,
        "pageIndex": session.doc.active_page_index(),
        "width": session.doc.width,
        "height": session.doc.height,
    }))
}

fn cmd_page_set_size(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetPageSizeParams = parse_params(id, params)?;
    if p.resolution.is_none() && p.orientation.is_none() && p.width.is_none() && p.height.is_none()
    {
        return Err(invalid(
            id,
            "provide `resolution`, `orientation`, `width`, or `height`",
        ));
    }
    validate_page_dimension_bounds(id, p.width, p.height)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let page_count = session.doc.page_count();
    let targets = if p.apply_all == Some(true) {
        (0..page_count).collect()
    } else if let Some(ref list) = p.pages {
        validate_page_selection(id, list, page_count)?
    } else {
        let idx = p.index.unwrap_or_else(|| session.doc.active_page_index());
        if idx >= page_count {
            return Err(invalid(id, "page index out of bounds"));
        }
        vec![idx]
    };
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let mut updated_count = 0usize;
    for idx in targets {
        let fallback = next
            .pages
            .get(idx)
            .map(|page| (page.width, page.height))
            .unwrap_or((next.width, next.height));
        let (tw, th) = resolve_requested_page_dimensions(
            fallback,
            p.resolution,
            p.orientation,
            p.width,
            p.height,
        );
        if next.set_page_dimensions(idx, tw, th) {
            updated_count += 1;
        }
    }
    if updated_count > 0 {
        session.history.record("Page Size", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({
        "updatedCount": updated_count,
        "pageIndex": session.doc.active_page_index(),
        "width": session.doc.width,
        "height": session.doc.height,
    }))
}

fn cmd_page_apply_template_all(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: ApplyTemplateAllParams = parse_params(id, params)?;
    if p.template.is_none()
        && p.color.is_none()
        && p.custom_template_id.is_none()
        && p.clear_custom_template.is_none()
    {
        return Err(invalid(
            id,
            "provide `template`, `color`, `custom_template_id`, or `clear_custom_template`",
        ));
    }
    let custom_tpl = match p.custom_template_id.as_deref() {
        Some(raw) => Some(find_custom_template(&session.prefs, id, raw)?.clone()),
        None => None,
    };
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    if let Some(ref tpl) = custom_tpl {
        next.apply_custom_template_to_all_pages(tpl)
            .map_err(|reason| invalid(id, reason))?;
        if let Some(c) = p.color {
            next.apply_template_to_all_pages(None, Some(c), false);
        }
    } else {
        next.apply_template_to_all_pages(
            p.template,
            p.color,
            p.clear_custom_template.unwrap_or(false),
        );
    }
    if next != *before {
        session
            .history
            .record("Apply Template to All Pages", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({
        "pageCount": session.doc.page_count(),
        "template": session.doc.paper_template,
        "color": session.doc.paper_color,
        "customTemplateId": session.doc.custom_template_id,
    }))
}

fn cmd_page_batch_duplicate(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: BatchPagesParams = parse_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let count = session.doc.page_count();
    let unique = validate_page_selection(id, &p.indices, count)?;
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let mut duplicated = 0usize;
    for &idx in unique.iter().rev() {
        if next.duplicate_page_at(idx).is_some() {
            duplicated += 1;
        }
    }
    if duplicated > 0 {
        session.history.record("Duplicate Pages", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({
        "duplicatedCount": duplicated,
        "pageCount": session.doc.page_count(),
        "pageIndex": session.doc.active_page_index(),
    }))
}

fn cmd_page_batch_remove(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: BatchPagesParams = parse_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let count = session.doc.page_count();
    let unique = validate_page_selection(id, &p.indices, count)?;
    if unique.len() >= count {
        return Err(invalid(
            id,
            "cannot remove every page; at least one page must remain",
        ));
    }
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let mut removed = 0usize;
    for &idx in unique.iter().rev() {
        if next.remove_page(idx) {
            removed += 1;
        }
    }
    if removed > 0 {
        session.history.record("Remove Pages", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }
    Ok(json!({
        "removedCount": removed,
        "pageCount": session.doc.page_count(),
        "pageIndex": session.doc.active_page_index(),
    }))
}

fn cmd_page_transfer(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: TransferPagesParams = parse_params(id, params)?;
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    session.sync_active_notebook_slot();

    let source_doc = (*session.doc).clone();
    let source_pages = source_doc.resolved_pages();
    let selected = validate_page_selection(id, &p.pages, source_pages.len())?;
    let move_pages = p.move_pages.unwrap_or(false);
    if move_pages && selected.len() >= source_pages.len() {
        return Err(invalid(
            id,
            "cannot move every page out of the source notebook; at least one page must remain",
        ));
    }

    let (target_idx, is_new_notebook, mut dst_doc) = match p.target_notebook {
        Some(target_idx) => {
            if target_idx >= session.notebooks.len() {
                return Err(invalid(id, "target notebook index out of bounds"));
            }
            if move_pages && target_idx == session.active_notebook {
                return Err(invalid(id, "cannot move pages into the same notebook"));
            }
            let existing = (*session.notebooks[target_idx]).clone();
            (target_idx, false, existing)
        }
        None => {
            if session.notebooks.len() >= MAX_NOTEBOOKS {
                return Err(invalid(id, "open notebook limit reached"));
            }
            let first_page = &source_pages[selected[0]];
            let mut created = Document::new(first_page.width, first_page.height);
            created.title = p
                .new_notebook_title
                .as_deref()
                .map(sanitize_notebook_title)
                .unwrap_or_else(|| {
                    format!("{} (Extract)", sanitize_notebook_title(&source_doc.title))
                });
            created.cover = source_doc.cover;
            created.folder_path = source_doc.folder_path.clone();
            let now = drawcraft_sync_protocol::now_unix_ms();
            created.created_at_ms = now;
            created.updated_at_ms = now;
            created.pages.clear();
            (session.notebooks.len(), true, created)
        }
    };

    let mut transferred = 0usize;
    for &page_idx in &selected {
        dst_doc
            .append_cloned_page(&source_pages[page_idx], &source_doc)
            .map_err(|reason| invalid(id, reason))?;
        transferred += 1;
    }
    let now = drawcraft_sync_protocol::now_unix_ms();
    if dst_doc.created_at_ms == 0 {
        dst_doc.created_at_ms = now;
    }
    dst_doc.updated_at_ms = now.max(dst_doc.updated_at_ms);
    dst_doc.load_active_from_pages();

    if is_new_notebook {
        session.notebooks.push(Arc::new(dst_doc));
        session
            .library
            .push_slot(None, true, session.notebooks.len());
        session.sync_mark_slot_dirty(target_idx);
    } else if target_idx == session.active_notebook {
        // Copying pages within the active notebook itself (when move_pages == false).
        let before = Arc::clone(&session.doc);
        session.history.record("Copy Pages", before);
        session.doc = Arc::new(dst_doc);
        session.mark_active_dirty();
    } else {
        session.notebooks[target_idx] = Arc::new(dst_doc);
        session
            .library
            .mark_dirty(target_idx, session.notebooks.len());
        session.sync_mark_slot_dirty(target_idx);
    }

    if move_pages {
        let before = Arc::clone(&session.doc);
        let mut next = (*session.doc).clone();
        for &page_idx in selected.iter().rev() {
            next.remove_page(page_idx);
        }
        next.updated_at_ms = now.max(next.updated_at_ms);
        session.history.record("Move Pages", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
    }

    session.sync_active_notebook_slot();
    session.bump_revision();
    let _ = session.save_library();
    let dest_pages = session
        .notebooks
        .get(target_idx)
        .map_or(0, |d| d.page_count());
    Ok(json!({
        "targetNotebookIndex": target_idx,
        "transferredCount": transferred,
        "moved": move_pages,
        "targetPageCount": dest_pages,
        "sourcePageCount": session.doc.page_count(),
    }))
}

fn resolve_cover_override(
    id: &str,
    cover: Option<NotebookCover>,
    cover_hex: Option<&str>,
) -> Result<Option<NotebookCover>, EngineError> {
    if let Some(hex) = cover_hex {
        let parsed = NotebookCover::from_hex(hex)
            .ok_or_else(|| invalid(id, format!("invalid cover_hex `{hex}` (expected #RRGGBB)")))?;
        return Ok(Some(parsed));
    }
    Ok(cover)
}

fn ensure_folder_tracked_in_prefs(session: &mut Session, folder_path: &str) {
    let clean = sanitize_folder_path(folder_path);
    if clean.is_empty() {
        return;
    }
    let mut updated = session.prefs.library_folders.clone();
    updated.push(clean);
    let normalized = sanitize_library_folders(&updated);
    if normalized != session.prefs.library_folders {
        session.prefs.library_folders = normalized;
        session.prefs = session.prefs.sanitized();
        let _ = session.save_prefs();
        session.sync_mark_prefs_dirty();
    }
}

fn folder_matches_prefix(candidate: &str, prefix: &str) -> Option<String> {
    folder_path_suffix(candidate, prefix)
}

fn cmd_notebook_create(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: CreateNotebookParams = parse_optional_params(id, params)?;
    let cover_override = resolve_cover_override(id, p.cover, p.cover_hex.as_deref())?;
    let custom_tpl = match p.custom_template_id.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            Some(find_custom_template(&session.prefs, id, raw)?.clone())
        }
        _ => None,
    };
    validate_page_dimension_bounds(id, p.width, p.height)?;
    if session.notebooks.len() >= MAX_NOTEBOOKS {
        return Err(invalid(id, "open notebook limit reached"));
    }
    session.sync_active_notebook_slot();
    session.cancel_live_stroke();
    session.cancel_live_eraser();

    let mut doc = p.preset.unwrap_or_default().create_document();
    let has_size_override = p.resolution.is_some()
        || p.orientation.is_some()
        || p.width.is_some()
        || p.height.is_some();
    if has_size_override {
        let fallback = custom_tpl
            .as_ref()
            .map(|tpl| (tpl.width.clamp(64, 8192), tpl.height.clamp(64, 8192)))
            .unwrap_or((doc.width, doc.height));
        let (w, h) = resolve_requested_page_dimensions(
            fallback,
            p.resolution,
            p.orientation,
            p.width,
            p.height,
        );
        doc.width = w;
        doc.height = h;
    }
    if let Some(ref title) = p.title {
        doc.title = sanitize_notebook_title(title);
    }
    if let Some(cover) = cover_override {
        doc.cover = cover;
    }
    if let Some(t) = p.template {
        doc.paper_template = t;
    }
    if let Some(c) = p.paper_color {
        doc.paper_color = c;
    }
    if let Some(ref raw_folder) = p.folder_path {
        doc.folder_path = sanitize_folder_path(raw_folder);
        ensure_folder_tracked_in_prefs(session, &doc.folder_path);
    }
    if let Some(fav) = p.favorite {
        doc.favorite = fav;
    }
    if let Some(ref raw_tags) = p.tags {
        doc.tags = sanitize_notebook_tags(raw_tags);
    }
    let now = drawcraft_sync_protocol::now_unix_ms();
    doc.id = uuid::Uuid::new_v4().to_string();
    doc.created_at_ms = now;
    doc.updated_at_ms = now;
    doc.meta_updated_at_ms = now;
    doc.sync_active_to_pages();
    if let Some(ref tpl) = custom_tpl {
        let override_dims = has_size_override.then_some((doc.width, doc.height));
        doc.apply_custom_template_to_page(0, tpl)
            .map_err(|reason| invalid(id, reason))?;
        if let Some(c) = p.paper_color {
            doc.paper_color = c;
            doc.sync_active_to_pages();
        }
        if let Some((w, h)) = override_dims {
            doc.set_page_dimensions(0, w, h);
        }
    }

    let title = doc.title.clone();
    let arc_doc = Arc::new(doc);
    session.notebooks.push(Arc::clone(&arc_doc));
    session
        .library
        .push_slot(None, true, session.notebooks.len());
    session.active_notebook = session.notebooks.len().saturating_sub(1);
    session.doc = arc_doc;
    session.history.clear();
    session.bump_revision();
    let _ = session.save_library();
    Ok(json!({
        "notebookIndex": session.active_notebook,
        "notebookCount": session.notebooks.len(),
        "title": title,
        "folderPath": session.doc.folder_path,
        "width": session.doc.width,
        "height": session.doc.height,
    }))
}

fn cmd_notebook_set_metadata(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetNotebookMetadataParams = parse_params(id, params)?;
    if p.folder_path.is_none()
        && p.favorite.is_none()
        && p.tags.is_none()
        && p.add_tag.is_none()
        && p.remove_tag.is_none()
    {
        return Err(invalid(
            id,
            "provide `folder_path`, `favorite`, `tags`, `add_tag`, or `remove_tag`",
        ));
    }
    session.sync_active_notebook_slot();
    let idx = p.index.unwrap_or(session.active_notebook);
    let Some(existing_arc) = session.notebooks.get(idx).cloned() else {
        return Err(invalid(id, "notebook index out of bounds"));
    };
    let mut next = (*existing_arc).clone();
    if let Some(ref raw_folder) = p.folder_path {
        next.folder_path = sanitize_folder_path(raw_folder);
        ensure_folder_tracked_in_prefs(session, &next.folder_path);
    }
    if let Some(fav) = p.favorite {
        next.favorite = fav;
    }
    if let Some(ref raw_tags) = p.tags {
        next.tags = sanitize_notebook_tags(raw_tags);
    }
    if let Some(ref raw_add) = p.add_tag
        && let Some(clean_tag) = sanitize_notebook_tag(raw_add)
    {
        let mut combined = next.tags.clone();
        combined.push(clean_tag);
        next.tags = sanitize_notebook_tags(&combined);
    }
    if let Some(ref raw_rem) = p.remove_tag
        && let Some(clean_rem) = sanitize_notebook_tag(raw_rem)
    {
        next.tags
            .retain(|tag| !tag.eq_ignore_ascii_case(&clean_rem));
    }

    if next != *existing_arc {
        let now = drawcraft_sync_protocol::now_unix_ms();
        if next.created_at_ms == 0 {
            next.created_at_ms = now;
        }
        next.updated_at_ms = now.max(next.updated_at_ms);
        next.meta_updated_at_ms = now.max(next.meta_updated_at_ms);
        let updated_arc = Arc::new(next);
        session.notebooks[idx] = Arc::clone(&updated_arc);
        if idx == session.active_notebook {
            session.doc = updated_arc;
        }
        session.library.mark_dirty(idx, session.notebooks.len());
        session.sync_mark_slot_dirty(idx);
        session.bump_revision();
        let _ = session.save_library();
    }
    let meta_doc = &session.notebooks[idx];
    Ok(json!({
        "notebookIndex": idx,
        "folderPath": meta_doc.folder_path,
        "favorite": meta_doc.favorite,
        "tags": meta_doc.tags,
    }))
}

fn cmd_notebook_merge(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: MergeNotebooksParams = parse_params(id, params)?;
    session.sync_active_notebook_slot();
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    if p.source_index >= session.notebooks.len() || p.target_index >= session.notebooks.len() {
        return Err(invalid(id, "notebook index out of bounds"));
    }
    if p.source_index == p.target_index {
        return Err(invalid(id, "source and target notebooks must be different"));
    }
    let source_doc = (*session.notebooks[p.source_index]).clone();
    let mut target_doc = (*session.notebooks[p.target_index]).clone();
    let src_pages = source_doc.resolved_pages();
    for page in &src_pages {
        target_doc
            .append_cloned_page(page, &source_doc)
            .map_err(|reason| invalid(id, reason))?;
    }
    let now = drawcraft_sync_protocol::now_unix_ms();
    if target_doc.created_at_ms == 0 {
        target_doc.created_at_ms = now;
    }
    target_doc.updated_at_ms = now.max(target_doc.updated_at_ms);
    let page_count = target_doc.page_count();
    let target_arc = Arc::new(target_doc);
    session.notebooks[p.target_index] = Arc::clone(&target_arc);
    if p.target_index == session.active_notebook {
        session.doc = target_arc;
        session.history.clear();
    }
    session
        .library
        .mark_dirty(p.target_index, session.notebooks.len());
    session.sync_mark_slot_dirty(p.target_index);

    let mut final_target_idx = p.target_index;
    let deleted_source = if p.delete_source.unwrap_or(false) && session.notebooks.len() > 1 {
        session
            .move_notebook_to_trash(p.source_index)
            .map_err(|reason| invalid(id, reason))?;
        session.sync_before_remove_slot(p.source_index);
        session.notebooks.remove(p.source_index);
        session.library.remove_slot(p.source_index);
        if p.source_index < final_target_idx {
            final_target_idx = final_target_idx.saturating_sub(1);
        }
        if session.active_notebook == p.source_index {
            session.active_notebook = final_target_idx;
        } else if p.source_index < session.active_notebook {
            session.active_notebook = session.active_notebook.saturating_sub(1);
        }
        session.doc = Arc::clone(&session.notebooks[session.active_notebook]);
        session.history.clear();
        true
    } else {
        false
    };

    session.bump_revision();
    let _ = session.save_library();
    Ok(json!({
        "targetNotebookIndex": final_target_idx,
        "pageCount": page_count,
        "mergedPages": src_pages.len(),
        "deletedSource": deleted_source,
        "notebookCount": session.notebooks.len(),
    }))
}

fn parse_optional_folder_color(
    cmd_id: &str,
    color: Option<Color>,
    color_hex: Option<&str>,
    clear: Option<bool>,
) -> Result<Option<Option<Color>>, EngineError> {
    if clear == Some(true) {
        return Ok(Some(None));
    }
    if color.is_some() || color_hex.is_some() {
        return Ok(Some(Some(parse_finite_opaque_color(
            cmd_id, color, color_hex,
        )?)));
    }
    Ok(None)
}

fn cmd_library_folder_create(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: CreateFolderParams = parse_params(id, params)?;
    let clean = sanitize_folder_path(&p.path);
    if clean.is_empty() {
        return Err(invalid(id, "folder path must not be empty"));
    }
    let color_update = parse_optional_folder_color(id, p.color, p.color_hex.as_deref(), None)?;
    let mut combined = session.prefs.library_folders.clone();
    combined.push(clean.clone());
    session.prefs.library_folders = sanitize_library_folders(&combined);
    if let Some(Some(color)) = color_update {
        session.prefs.set_folder_color(&clean, Some(color));
    }
    if let Some(ref icon) = p.icon {
        session.prefs.set_folder_icon(&clean, Some(icon));
    }
    session.prefs = session.prefs.sanitized();
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "path": clean,
        "folders": session.prefs.library_folders,
        "color": session.prefs.folder_color(&clean),
        "icon": session.prefs.folder_icon(&clean),
    }))
}

fn cmd_library_folder_rename(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: RenameFolderParams = parse_params(id, params)?;
    let old_clean = sanitize_folder_path(&p.old_path);
    let new_clean = sanitize_folder_path(&p.new_path);
    if old_clean.is_empty() || new_clean.is_empty() {
        return Err(invalid(id, "old_path and new_path must not be empty"));
    }
    let color_update =
        parse_optional_folder_color(id, p.color, p.color_hex.as_deref(), p.clear_color)?;
    session.sync_active_notebook_slot();

    let rewrite = |candidate: &str| -> Option<String> {
        let suffix = folder_matches_prefix(candidate, &old_clean)?;
        Some(if suffix.is_empty() {
            new_clean.clone()
        } else {
            format!("{new_clean}/{suffix}")
        })
    };

    let mut updated_folders = Vec::with_capacity(session.prefs.library_folders.len() + 1);
    for folder in &session.prefs.library_folders {
        updated_folders.push(rewrite(folder).unwrap_or_else(|| folder.clone()));
    }
    updated_folders.push(new_clean.clone());
    session.prefs.library_folders = sanitize_library_folders(&updated_folders);
    session.prefs.remap_folder_colors(&old_clean, &new_clean);
    session.prefs.remap_folder_icons(&old_clean, &new_clean);
    session.prefs.remap_folder_favorites(&old_clean, &new_clean);
    if let Some(next_color) = color_update {
        session.prefs.set_folder_color(&new_clean, next_color);
    }
    if p.clear_icon == Some(true) {
        session.prefs.set_folder_icon(&new_clean, None);
    } else if let Some(ref icon) = p.icon {
        session.prefs.set_folder_icon(&new_clean, Some(icon));
    }
    session.prefs = session.prefs.sanitized();
    let _ = session.save_prefs();

    let count = session.notebooks.len();
    let now = drawcraft_sync_protocol::now_unix_ms();
    let mut moved_notebooks = 0usize;
    for idx in 0..count {
        if let Some(replaced) = rewrite(&session.notebooks[idx].folder_path) {
            let mut next = (*session.notebooks[idx]).clone();
            next.folder_path = sanitize_folder_path(&replaced);
            next.updated_at_ms = now.max(next.updated_at_ms);
            let arc = Arc::new(next);
            session.notebooks[idx] = Arc::clone(&arc);
            if idx == session.active_notebook {
                session.doc = arc;
            }
            session.library.mark_dirty(idx, count);
            session.sync_mark_slot_dirty(idx);
            moved_notebooks += 1;
        }
    }
    if moved_notebooks > 0 {
        let _ = session.save_library();
    }
    session.bump_revision();
    Ok(json!({
        "oldPath": old_clean,
        "newPath": new_clean,
        "updatedNotebooks": moved_notebooks,
        "folders": session.prefs.library_folders,
        "color": session.prefs.folder_color(&new_clean),
    }))
}

fn cmd_library_folder_delete(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DeleteFolderParams = parse_params(id, params)?;
    let target = sanitize_folder_path(&p.path);
    if target.is_empty() {
        return Err(invalid(id, "folder path must not be empty"));
    }
    let mut dest = p
        .move_notebooks_to
        .as_deref()
        .map(sanitize_folder_path)
        .unwrap_or_default();
    if folder_matches_prefix(&dest, &target).is_some() {
        dest = target
            .rsplit_once('/')
            .map_or(String::new(), |(parent, _)| parent.to_string());
    }
    session.sync_active_notebook_slot();

    session
        .prefs
        .library_folders
        .retain(|folder| folder_matches_prefix(folder, &target).is_none());
    session.prefs.remove_folder_colors(&target);
    session.prefs.remove_folder_icons(&target);
    session.prefs.remove_folder_favorites(&target);
    session.prefs = session.prefs.sanitized();
    let _ = session.save_prefs();

    let count = session.notebooks.len();
    let now = drawcraft_sync_protocol::now_unix_ms();
    let mut relocated = 0usize;
    for idx in 0..count {
        if folder_matches_prefix(&session.notebooks[idx].folder_path, &target).is_some() {
            let mut next = (*session.notebooks[idx]).clone();
            next.folder_path = dest.clone();
            next.updated_at_ms = now.max(next.updated_at_ms);
            let arc = Arc::new(next);
            session.notebooks[idx] = Arc::clone(&arc);
            if idx == session.active_notebook {
                session.doc = arc;
            }
            session.library.mark_dirty(idx, count);
            session.sync_mark_slot_dirty(idx);
            relocated += 1;
        }
    }
    if relocated > 0 {
        let _ = session.save_library();
    }
    session.bump_revision();
    Ok(json!({
        "deleted": target,
        "movedNotebooks": relocated,
        "destinationFolder": dest,
        "folders": session.prefs.library_folders,
    }))
}

fn cmd_library_folder_set_favorite(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetFolderFavoriteParams = parse_params(id, params)?;
    let clean = sanitize_folder_path(&p.path);
    if clean.is_empty() {
        return Err(invalid(id, "folder path must not be empty"));
    }
    if p.favorite {
        ensure_folder_tracked_in_prefs(session, &clean);
    }
    let changed = session.prefs.set_folder_favorite(&clean, p.favorite);
    session.prefs = session.prefs.sanitized();
    if changed {
        let _ = session.save_prefs();
        session.bump_revision();
    }
    Ok(json!({
        "path": clean,
        "favorite": session.prefs.is_folder_favorite(&clean),
    }))
}

fn cmd_library_folder_set_color(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetFolderColorParams = parse_params(id, params)?;
    let clean = sanitize_folder_path(&p.path);
    if clean.is_empty() {
        return Err(invalid(id, "folder path must not be empty"));
    }
    let Some(next_color) =
        parse_optional_folder_color(id, p.color, p.color_hex.as_deref(), p.clear)?
    else {
        return Err(invalid(
            id,
            "provide `color`, `color_hex`, or `clear: true`",
        ));
    };

    let already_colored = session.prefs.folder_color(&clean).is_some();
    if next_color.is_some()
        && !already_colored
        && session.prefs.library_folder_colors.len() >= MAX_LIBRARY_FOLDERS
    {
        return Err(invalid(id, "folder color limit reached"));
    }
    if next_color.is_some() {
        ensure_folder_tracked_in_prefs(session, &clean);
    }
    let changed = session.prefs.set_folder_color(&clean, next_color);
    session.prefs = session.prefs.sanitized();
    if changed {
        let _ = session.save_prefs();
        session.bump_revision();
    }
    let saved = session.prefs.folder_color(&clean);
    Ok(json!({
        "path": clean,
        "color": saved,
        "color_hex": saved.map(Color::to_hex),
    }))
}

fn cmd_library_tag_set_color(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetTagColorParams = parse_params(id, params)?;
    let Some(clean) = sanitize_notebook_tag(&p.tag) else {
        return Err(invalid(id, "tag must not be empty"));
    };
    let Some(next_color) =
        parse_optional_folder_color(id, p.color, p.color_hex.as_deref(), p.clear)?
    else {
        return Err(invalid(
            id,
            "provide `color`, `color_hex`, or `clear: true`",
        ));
    };

    let already_colored = session.prefs.tag_color(&clean).is_some();
    if next_color.is_some()
        && !already_colored
        && session.prefs.library_tag_colors.len() >= MAX_LIBRARY_TAG_COLORS
    {
        return Err(invalid(id, "tag color limit reached"));
    }
    let changed = session.prefs.set_tag_color(&clean, next_color);
    session.prefs = session.prefs.sanitized();
    if changed {
        let _ = session.save_prefs();
        session.bump_revision();
    }
    let saved = session.prefs.tag_color(&clean);
    Ok(json!({
        "tag": clean,
        "color": saved,
        "color_hex": saved.map(Color::to_hex),
    }))
}

fn cmd_library_set_view_prefs(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetLibraryViewPrefsParams = parse_params(id, params)?;
    if p.view_mode.is_none() && p.sort_by.is_none() && p.sort_ascending.is_none() {
        return Err(invalid(
            id,
            "provide `view_mode`, `sort_by`, and/or `sort_ascending`",
        ));
    }
    if let Some(mode) = p.view_mode {
        session.prefs.library_view_mode = mode;
    }
    if let Some(sort) = p.sort_by {
        session.prefs.library_sort_by = sort;
    }
    if let Some(asc) = p.sort_ascending {
        session.prefs.library_sort_ascending = asc;
    }
    session.prefs = session.prefs.sanitized();
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "viewMode": session.prefs.library_view_mode,
        "sortBy": session.prefs.library_sort_by,
        "sortAscending": session.prefs.library_sort_ascending,
    }))
}

fn extract_binary_payload(
    id: &str,
    path: Option<&str>,
    bytes: Option<&[u8]>,
    base64_bytes: Option<&str>,
    max_bytes: Option<usize>,
) -> Result<Vec<u8>, EngineError> {
    if let Some(b) = bytes {
        if max_bytes.is_some_and(|limit| b.len() > limit) {
            return Err(invalid(id, "payload exceeds maximum size limit"));
        }
        return Ok(b.to_vec());
    }
    if let Some(b64) = base64_bytes {
        return match max_bytes {
            Some(limit) => base64_decode_bytes_with_limit(b64, limit),
            None => base64_decode_bytes_unbounded(b64),
        }
        .map_err(|reason| invalid(id, reason));
    }
    if let Some(raw_path) = path {
        let clean = sanitize_io_path(id, raw_path)?;
        if let Some(limit) = max_bytes
            && let Ok(meta) = std::fs::metadata(&clean)
            && meta.len() > limit as u64
        {
            return Err(invalid(id, "file exceeds maximum size limit"));
        }
        let raw = std::fs::read(&clean)
            .map_err(|err| invalid(id, format!("failed to read file: {err}")))?;
        if max_bytes.is_some_and(|limit| raw.len() > limit) {
            return Err(invalid(id, "file exceeds maximum size limit"));
        }
        return Ok(raw);
    }
    Err(invalid(id, "provide `path`, `bytes`, or `base64_bytes`"))
}

fn shrink_png_to_limit(id: &str, png: Vec<u8>, max_bytes: usize) -> Result<Vec<u8>, EngineError> {
    if png.len() <= max_bytes {
        return Ok(png);
    }
    let decoded = image::load_from_memory(&png)
        .map_err(|e| invalid(id, format!("failed re-reading encoded template PNG: {e}")))?;
    let mut rgba = decoded.to_rgba8();
    for _ in 0..6 {
        let next_w = ((rgba.width() * 3) / 4).max(256).min(rgba.width());
        let next_h = ((rgba.height() * 3) / 4).max(256).min(rgba.height());
        if next_w == rgba.width() && next_h == rgba.height() {
            break;
        }
        rgba =
            image::imageops::resize(&rgba, next_w, next_h, image::imageops::FilterType::Triangle);
        let encoded = drawcraft_raster::png_encode(rgba.width(), rgba.height(), rgba.as_raw())
            .map_err(|e| invalid(id, format!("failed encoding PNG template: {e}")))?;
        if encoded.len() <= max_bytes {
            return Ok(encoded);
        }
    }
    Err(invalid(
        id,
        "template image exceeds maximum 3 MiB PNG limit even after scaling",
    ))
}

fn template_page_size_for_pixels(w: u32, h: u32) -> (u32, u32) {
    let wf = f64::from(w.max(1));
    let hf = f64::from(h.max(1));
    let scale = 1600.0 / wf.max(hf);
    let pw = (wf * scale).round().clamp(400.0, 2400.0) as u32;
    let ph = (hf * scale).round().clamp(400.0, 2400.0) as u32;
    (pw, ph)
}

fn next_custom_template_id(existing: &[CustomPaperTemplate]) -> String {
    let mut seq = 1usize;
    loop {
        let candidate = format!("tpl_{seq}");
        if !existing.iter().any(|t| t.id == candidate) {
            return candidate;
        }
        seq = seq.saturating_add(1);
    }
}

fn cmd_template_save_from_image(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SaveImageTemplateParams = parse_params(id, params)?;
    if session.prefs.custom_templates.len() >= MAX_CUSTOM_TEMPLATES {
        return Err(invalid(
            id,
            format!("reached the {MAX_CUSTOM_TEMPLATES} custom paper templates limit"),
        ));
    }
    let raw = extract_binary_payload(
        id,
        p.path.as_deref(),
        p.bytes.as_deref(),
        p.base64_bytes.as_deref(),
        Some(MAX_RAW_IMAGE_BYTES),
    )?;
    let (px_w, px_h, png) = decode_and_normalize_image(id, &raw)?;
    let png = shrink_png_to_limit(id, png, MAX_CUSTOM_TEMPLATE_IMAGE_BYTES)?;
    let (pw, ph) = template_page_size_for_pixels(px_w, px_h);
    let tpl_id = next_custom_template_id(&session.prefs.custom_templates);
    let tpl = CustomPaperTemplate {
        id: tpl_id,
        name: p.name,
        kind: CustomTemplateKind::Image,
        width: pw,
        height: ph,
        pdf_page_index: 0,
        bytes: Arc::new(png),
    };
    let clean_tpl = tpl
        .sanitized()
        .ok_or_else(|| invalid(id, "custom image template failed validation"))?;
    let mut next_prefs = session.prefs.clone();
    next_prefs.custom_templates.push(clean_tpl.clone());
    session.prefs = next_prefs.sanitized();
    if !session
        .prefs
        .custom_templates
        .iter()
        .any(|t| t.id == clean_tpl.id)
    {
        return Err(invalid(
            id,
            "custom templates storage budget exceeded; remove older custom templates first",
        ));
    }
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "id": clean_tpl.id,
        "name": clean_tpl.name,
        "kind": clean_tpl.kind,
        "width": clean_tpl.width,
        "height": clean_tpl.height,
        "count": session.prefs.custom_templates.len(),
    }))
}

fn cmd_template_save_from_pdf(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SavePdfTemplateParams = parse_params(id, params)?;
    if session.prefs.custom_templates.len() >= MAX_CUSTOM_TEMPLATES {
        return Err(invalid(
            id,
            format!("reached the {MAX_CUSTOM_TEMPLATES} custom paper templates limit"),
        ));
    }
    let raw = extract_binary_payload(
        id,
        p.path.as_deref(),
        p.bytes.as_deref(),
        p.base64_bytes.as_deref(),
        None,
    )?;
    let arc_pdf = Arc::new(raw);
    let summary = inspect_pdf(Arc::clone(&arc_pdf), usize::MAX)
        .map_err(|err| invalid(id, err.to_string()))?;
    let page_index = p.page_index.unwrap_or(0);
    let metrics = summary
        .pages
        .get(page_index)
        .copied()
        .ok_or_else(|| invalid(id, "PDF template page index out of bounds"))?;
    let (pw, ph) = (metrics.page_width, metrics.page_height);
    let tpl_id = next_custom_template_id(&session.prefs.custom_templates);

    let tpl = if arc_pdf.len() <= MAX_CUSTOM_TEMPLATE_PDF_BYTES {
        CustomPaperTemplate {
            id: tpl_id,
            name: p.name,
            kind: CustomTemplateKind::Pdf,
            width: pw,
            height: ph,
            pdf_page_index: page_index,
            bytes: arc_pdf,
        }
    } else {
        // Oversized multi-page PDF: rasterize the selected page crisply to keep template storage bounded.
        let rendered = drawcraft_compose::pdf::render_pdf_page_exact_rgba(
            arc_pdf,
            page_index,
            pw.min(1600),
            ph.min(1600),
        )
        .map_err(|err| invalid(id, format!("failed rasterizing PDF template page: {err}")))?;
        let png = drawcraft_raster::png_encode(rendered.width, rendered.height, &rendered.rgba)
            .map_err(|e| invalid(id, format!("failed encoding PDF template PNG: {e}")))?;
        let png = shrink_png_to_limit(id, png, MAX_CUSTOM_TEMPLATE_IMAGE_BYTES)?;
        CustomPaperTemplate {
            id: tpl_id,
            name: p.name,
            kind: CustomTemplateKind::Image,
            width: pw,
            height: ph,
            pdf_page_index: 0,
            bytes: Arc::new(png),
        }
    };

    let clean_tpl = tpl
        .sanitized()
        .ok_or_else(|| invalid(id, "custom PDF template failed validation"))?;
    let mut next_prefs = session.prefs.clone();
    next_prefs.custom_templates.push(clean_tpl.clone());
    session.prefs = next_prefs.sanitized();
    if !session
        .prefs
        .custom_templates
        .iter()
        .any(|t| t.id == clean_tpl.id)
    {
        return Err(invalid(
            id,
            "custom templates storage budget exceeded; remove older custom templates first",
        ));
    }
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "id": clean_tpl.id,
        "name": clean_tpl.name,
        "kind": clean_tpl.kind,
        "width": clean_tpl.width,
        "height": clean_tpl.height,
        "count": session.prefs.custom_templates.len(),
    }))
}

fn cmd_template_delete(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DeleteCustomTemplateParams = parse_params(id, params)?;
    let clean_id = sanitize_template_id(&p.id);
    let Some(pos) = session
        .prefs
        .custom_templates
        .iter()
        .position(|t| t.id == clean_id)
    else {
        return Err(invalid(id, format!("unknown custom template `{clean_id}`")));
    };
    session.prefs.custom_templates.remove(pos);
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "deleted": clean_id,
        "count": session.prefs.custom_templates.len(),
    }))
}

fn cmd_notebook_select(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SelectIndexParams = parse_params(id, params)?;
    if !session.switch_notebook(p.index) {
        return Err(invalid(id, "notebook index out of bounds"));
    }
    Ok(json!({
        "notebookIndex": session.active_notebook,
        "title": session.doc.title
    }))
}

fn cmd_notebook_rename(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: RenameNotebookParams = parse_params(id, params)?;
    let cover_override = resolve_cover_override(id, p.cover, p.cover_hex.as_deref())?;
    if p.title.is_none() && cover_override.is_none() {
        return Err(invalid(id, "provide `title`, `cover`, and/or `cover_hex`"));
    }
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    if let Some(ref raw) = p.title {
        next.title = sanitize_notebook_title(raw);
    }
    if let Some(cover) = cover_override {
        next.cover = cover;
    }
    if next != *before {
        let now = drawcraft_sync_protocol::now_unix_ms();
        next.meta_updated_at_ms = now.max(next.meta_updated_at_ms);
        next.updated_at_ms = now.max(next.updated_at_ms);
        session.history.record("Notebook Metadata", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
        let _ = session.save_library();
    }
    Ok(json!({ "title": session.doc.title, "cover": session.doc.cover }))
}

fn cmd_notebook_delete(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: OptionalIndexParams = parse_optional_params(id, params)?;
    session.sync_active_notebook_slot();
    let idx = p.index.unwrap_or(session.active_notebook);
    if idx >= session.notebooks.len() {
        return Err(invalid(id, "notebook index out of bounds"));
    }
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    session
        .move_notebook_to_trash(idx)
        .map_err(|reason| invalid(id, reason))?;
    session.sync_before_remove_slot(idx);
    session.notebooks.remove(idx);
    session.library.remove_slot(idx);
    if session.notebooks.is_empty() {
        session.active_notebook = 0;
        let mut blank = drawcraft_doc::Document::default();
        blank.sync_active_to_pages();
        session.doc = Arc::new(blank);
    } else {
        if idx < session.active_notebook {
            session.active_notebook = session.active_notebook.saturating_sub(1);
        } else if session.active_notebook >= session.notebooks.len() {
            session.active_notebook = session.notebooks.len().saturating_sub(1);
        }
        session.doc = Arc::clone(&session.notebooks[session.active_notebook]);
    }
    session.history.clear();
    session.bump_revision();
    Ok(json!({
        "notebookIndex": session.active_notebook,
        "notebookCount": session.notebooks.len(),
        "trashCount": session.trashed_notebooks.len()
    }))
}

fn cmd_notebook_restore(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SelectIndexParams = parse_params(id, params)?;
    session
        .restore_trashed_notebook(p.index)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({
        "notebookIndex": session.active_notebook,
        "notebookCount": session.notebooks.len(),
        "trashCount": session.trashed_notebooks.len(),
        "title": session.doc.title
    }))
}

fn cmd_notebook_trash_cleanup(session: &mut Session) -> Result<Value, EngineError> {
    let before = session.trashed_notebooks.len();
    session.cleanup_expired_trash();
    Ok(json!({
        "removed": before.saturating_sub(session.trashed_notebooks.len()),
        "trashCount": session.trashed_notebooks.len()
    }))
}

fn cmd_notebook_save(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SaveOrExportParams = parse_optional_params(id, params)?;
    let snapshot = build_notebook_subset_snapshot(&session.doc, id, p.page, p.pages.as_deref())?;
    if p.in_memory == Some(true) {
        let json_text =
            save_document(&snapshot).map_err(|err| invalid(id, format!("save failed: {err}")))?;
        session.last_export_status = Some(format!(
            "Saved notebook ({} pages, {} bytes in memory).",
            snapshot.page_count(),
            json_text.len()
        ));
        return Ok(json!({ "bytes": json_text.len(), "json": json_text }));
    }
    let bytes =
        save_document_bytes(&snapshot).map_err(|err| invalid(id, format!("save failed: {err}")))?;
    let written = write_export_bytes(id, p.path.as_deref(), &bytes)?;
    session.last_export_status = Some(format!("Saved notebook to `{written}`."));
    Ok(json!({ "path": written, "bytes": bytes.len() }))
}

fn cmd_notebook_load(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: LoadNotebookParams = parse_params(id, params)?;
    let doc = match (p.json, p.bytes, p.path) {
        (Some(text), _, _) => load_document(&text)
            .map_err(|err| invalid(id, format!("invalid .dcraft notebook: {err}")))?,
        (None, Some(raw), _) => decode_notebook_bytes(id, &raw)?,
        (None, None, Some(path_str)) => {
            let clean = sanitize_io_path(id, &path_str)?;
            let raw = std::fs::read(&clean)
                .map_err(|err| invalid(id, format!("failed to read notebook file: {err}")))?;
            decode_notebook_bytes(id, &raw)?
        }
        (None, None, None) => {
            return Err(invalid(
                id,
                "provide `path`, `bytes`, or `json` to open a notebook",
            ));
        }
    };
    Ok(insert_loaded_document(
        session,
        doc,
        p.source_name.as_deref(),
        p.replace_active == Some(true),
        true,
    ))
}

fn decode_notebook_bytes(id: &str, raw: &[u8]) -> Result<drawcraft_doc::Document, EngineError> {
    load_document_bytes(raw).map_err(|err| invalid(id, format!("invalid .dcraft notebook: {err}")))
}

fn insert_loaded_document(
    session: &mut Session,
    mut doc: drawcraft_doc::Document,
    source_name: Option<&str>,
    replace_active: bool,
    persist_now: bool,
) -> Value {
    let clean_existing_id = drawcraft_doc::sanitize_notebook_id(&doc.id);
    if clean_existing_id.is_empty()
        || (!replace_active
            && session
                .notebooks
                .iter()
                .any(|existing| existing.id == clean_existing_id))
    {
        doc.id = uuid::Uuid::new_v4().to_string();
    } else {
        doc.id = clean_existing_id;
    }
    doc.title = sanitize_notebook_title(&doc.title);
    doc.sync_active_to_pages();
    doc.load_active_from_pages();

    session.sync_active_notebook_slot();
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let title = doc.title.clone();
    let page_count = doc.page_count();
    let arc_doc = Arc::new(doc);

    if (replace_active && !session.notebooks.is_empty()) || session.notebooks.len() >= MAX_NOTEBOOKS
    {
        session.notebooks[session.active_notebook] = Arc::clone(&arc_doc);
        session
            .library
            .mark_dirty(session.active_notebook, session.notebooks.len());
        session.sync_mark_slot_dirty(session.active_notebook);
    } else {
        session.notebooks.push(Arc::clone(&arc_doc));
        session
            .library
            .push_slot(None, true, session.notebooks.len());
        session.active_notebook = session.notebooks.len().saturating_sub(1);
    }
    session.doc = arc_doc;
    session.history.clear();
    session.bump_revision();
    if persist_now {
        let _ = session.save_library();
    }
    let clean_source = source_name.map(str::trim).filter(|s| !s.is_empty());
    session.last_export_status = Some(match clean_source {
        Some(name) => format!("Opened \"{title}\" from `{name}` ({page_count} pages)."),
        None => format!("Opened \"{title}\" ({page_count} pages)."),
    });
    json!({
        "notebookIndex": session.active_notebook,
        "title": title,
        "pageCount": page_count
    })
}

/// Validated and parsed PDF import payload ready to commit into a [`Session`] without further I/O.
pub struct PreparedPdfImport {
    pub bytes: Arc<Vec<u8>>,
    pub summary: PdfDocumentSummary,
    pub title: Option<String>,
    pub source_name: Option<String>,
    pub mode: PdfImportMode,
    pub after_page: Option<usize>,
}

/// Reads and inspects a PDF from disk off the UI thread so importing large PDFs does not block frames.
pub fn prepare_pdf_import_from_path(
    path: &Path,
    source_name: Option<String>,
    mode: PdfImportMode,
) -> Result<PreparedPdfImport, EngineError> {
    let raw = std::fs::read(path)
        .map_err(|err| invalid("pdf.import", format!("failed to read PDF file: {err}")))?;
    let resolved_name = source_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
    prepare_pdf_import_from_bytes(raw, resolved_name, mode)
}

/// Validates and inspects raw PDF bytes off the UI thread so multi-page imports stay responsive.
pub fn prepare_pdf_import_from_bytes(
    bytes: Vec<u8>,
    source_name: Option<String>,
    mode: PdfImportMode,
) -> Result<PreparedPdfImport, EngineError> {
    let arc_bytes = Arc::new(bytes);
    let summary = inspect_pdf(Arc::clone(&arc_bytes), usize::MAX)
        .map_err(|err| invalid("pdf.import", err.to_string()))?;
    if summary.pages.is_empty() {
        return Err(invalid(
            "pdf.import",
            "PDF file does not contain any pages.",
        ));
    }
    Ok(PreparedPdfImport {
        bytes: arc_bytes,
        summary,
        title: None,
        source_name: source_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        mode,
        after_page: None,
    })
}

fn prepare_pdf_import_from_params(
    id: &str,
    p: PdfImportParams,
) -> Result<(PreparedPdfImport, bool), EngineError> {
    let mode = p.mode.unwrap_or_default();
    let replace_active = p.replace_active.unwrap_or(false);
    let mut source_name = p
        .source_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let bytes = if let Some(raw) = p.bytes {
        Arc::new(raw)
    } else if let Some(b64) = p.base64_bytes {
        let raw = base64_decode_bytes_unbounded(&b64).map_err(|why| invalid(id, why))?;
        Arc::new(raw)
    } else if let Some(path_str) = p.path {
        let clean = sanitize_io_path(id, &path_str)?;
        let raw = std::fs::read(&clean)
            .map_err(|err| invalid(id, format!("failed to read PDF file: {err}")))?;
        if source_name.is_none() {
            source_name = clean
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        Arc::new(raw)
    } else {
        return Err(invalid(
            id,
            "provide `path`, `bytes`, or `base64_bytes` to import a PDF",
        ));
    };

    let summary =
        inspect_pdf(Arc::clone(&bytes), usize::MAX).map_err(|err| invalid(id, err.to_string()))?;
    if summary.pages.is_empty() {
        return Err(invalid(id, "PDF file does not contain any pages."));
    }
    Ok((
        PreparedPdfImport {
            bytes,
            summary,
            title: p.title,
            source_name,
            mode,
            after_page: p.after_page,
        },
        replace_active,
    ))
}

fn strip_pdf_extension(name: &str) -> &str {
    let trimmed = name.trim();
    if trimmed.len() >= 4 && trimmed[trimmed.len() - 4..].eq_ignore_ascii_case(".pdf") {
        &trimmed[..trimmed.len() - 4]
    } else {
        trimmed
    }
}

fn resolve_imported_pdf_title(
    explicit: Option<&str>,
    metadata_title: Option<&str>,
    source_name: Option<&str>,
) -> String {
    if let Some(t) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return sanitize_notebook_title(t);
    }
    if let Some(t) = metadata_title.map(str::trim).filter(|s| !s.is_empty()) {
        return sanitize_notebook_title(t);
    }
    if let Some(src) = source_name
        .map(strip_pdf_extension)
        .filter(|s| !s.is_empty())
    {
        return sanitize_notebook_title(src);
    }
    "Imported PDF".to_string()
}

fn build_pdf_backdrop_pages(pdf_index: usize, summary: &PdfDocumentSummary) -> Vec<NotebookPage> {
    summary
        .pages
        .iter()
        .enumerate()
        .map(|(page_index, metrics)| {
            let mut page = NotebookPage::new(
                metrics.page_width,
                metrics.page_height,
                PaperTemplate::Blank,
                PaperColor::PureWhite,
            );
            page.pdf_backdrop = Some(PdfPageBackdrop {
                pdf_index,
                page_index,
            });
            page
        })
        .collect()
}

fn commit_pdf_as_notebook(
    session: &mut Session,
    id: &str,
    req: PreparedPdfImport,
    replace_active: bool,
    persist_now: bool,
) -> Result<Value, EngineError> {
    let first = req
        .summary
        .pages
        .first()
        .copied()
        .ok_or_else(|| invalid(id, "PDF file does not contain any pages."))?;
    let source_ref = req.source_name.as_deref();
    let pdf_name = sanitize_pdf_name(source_ref.unwrap_or("Imported.pdf"));
    let title = resolve_imported_pdf_title(
        req.title.as_deref(),
        req.summary.title.as_deref(),
        source_ref,
    );

    let mut doc = Document::new(first.page_width, first.page_height);
    doc.title = title;
    doc.cover = NotebookCover::SlateGraphite;
    doc.paper_template = PaperTemplate::Blank;
    doc.paper_color = PaperColor::PureWhite;
    doc.pdf_documents = vec![EmbeddedPdf {
        name: pdf_name.clone(),
        bytes: req.bytes,
    }];
    doc.pages = build_pdf_backdrop_pages(0, &req.summary);
    doc.current_page = 0;
    doc.load_active_from_pages();

    let mut out = insert_loaded_document(session, doc, source_ref, replace_active, persist_now);
    let page_count = session.doc.page_count();
    session.last_export_status = Some(format!(
        "Imported PDF \"{}\" ({page_count} {}).",
        session.doc.title,
        if page_count == 1 { "page" } else { "pages" }
    ));
    if let Some(obj) = out.as_object_mut() {
        obj.insert("mode".to_string(), json!("new_notebook"));
        obj.insert("pdfName".to_string(), json!(pdf_name));
        obj.insert("insertedPages".to_string(), json!(page_count));
    }
    Ok(out)
}

fn find_existing_embedded_pdf(doc: &Document, new_bytes: &[u8]) -> Option<usize> {
    doc.pdf_documents
        .iter()
        .position(|pdf| pdf.bytes.as_slice() == new_bytes)
}

fn commit_pdf_into_active(
    session: &mut Session,
    req: PreparedPdfImport,
    persist_now: bool,
) -> Result<Value, EngineError> {
    let existing_pdf_idx = find_existing_embedded_pdf(&session.doc, &req.bytes);
    let pdf_name = sanitize_pdf_name(req.source_name.as_deref().unwrap_or("Imported.pdf"));
    session.cancel_live_stroke();
    session.cancel_live_eraser();
    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    next.sync_active_to_pages();

    let pdf_index = match existing_pdf_idx {
        Some(idx) => idx,
        None => {
            let idx = next.pdf_documents.len();
            next.pdf_documents.push(EmbeddedPdf {
                name: pdf_name.clone(),
                bytes: req.bytes,
            });
            idx
        }
    };
    let insert_at = req
        .after_page
        .map_or_else(
            || next.current_page.saturating_add(1),
            |idx| idx.saturating_add(1),
        )
        .min(next.pages.len());
    let new_pages = build_pdf_backdrop_pages(pdf_index, &req.summary);
    let inserted_pages = new_pages.len();
    next.pages.splice(insert_at..insert_at, new_pages);
    next.current_page = insert_at;
    next.load_active_from_pages();

    session.history.record("Import PDF Pages", before);
    session.doc = Arc::new(next);
    session.sync_active_notebook_slot();
    session.mark_active_dirty();
    session.bump_revision();
    if persist_now {
        let _ = session.save_library();
    }
    session.last_export_status = Some(format!(
        "Inserted {inserted_pages} PDF {} from `{pdf_name}`.",
        if inserted_pages == 1 { "page" } else { "pages" }
    ));

    Ok(json!({
        "mode": "insert_pages",
        "notebookIndex": session.active_notebook,
        "title": session.doc.title,
        "pdfName": pdf_name,
        "pdfIndex": pdf_index,
        "startPageIndex": insert_at,
        "insertedPages": inserted_pages,
        "pageCount": session.doc.page_count()
    }))
}

fn cmd_pdf_import(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: PdfImportParams = parse_params(id, params)?;
    let (prepared, replace_active) = prepare_pdf_import_from_params(id, p)?;
    session.apply_prepared_pdf_import(prepared, replace_active, true)
}

impl Session {
    /// Commits a pre-parsed [`PreparedPdfImport`] into the session.
    pub fn apply_prepared_pdf_import(
        &mut self,
        prepared: PreparedPdfImport,
        replace_active: bool,
        persist_now: bool,
    ) -> Result<Value, EngineError> {
        match prepared.mode {
            PdfImportMode::NewNotebook => {
                commit_pdf_as_notebook(self, "pdf.import", prepared, replace_active, persist_now)
            }
            PdfImportMode::InsertPages => commit_pdf_into_active(self, prepared, persist_now),
        }
    }

    /// Imports a PDF payload directly from raw bytes into a new notebook or the active notebook.
    pub fn import_pdf_bytes(
        &mut self,
        bytes: &[u8],
        source_name: Option<&str>,
        mode: PdfImportMode,
        replace_active: bool,
    ) -> Result<Value, EngineError> {
        let prepared =
            prepare_pdf_import_from_bytes(bytes.to_vec(), source_name.map(str::to_string), mode)?;
        self.apply_prepared_pdf_import(prepared, replace_active, true)
    }

    /// Inspects metadata and page dimensions of an in-memory PDF payload.
    pub fn inspect_pdf_bytes(bytes: Arc<Vec<u8>>) -> Result<PdfDocumentSummary, PdfError> {
        inspect_pdf(bytes, usize::MAX)
    }

    /// Opens a PDF once and passes a reusable [`PdfPageRenderer`] to `f`.
    pub fn with_pdf_renderer<R, F>(bytes: Arc<Vec<u8>>, f: F) -> Result<R, PdfError>
    where
        F: FnOnce(&mut PdfPageRenderer<'_>) -> R,
    {
        with_pdf_renderer(bytes, f)
    }

    /// Rasterizes one page of an in-memory PDF into opaque RGBA8 pixels within `max_w x max_h`.
    pub fn render_pdf_page_rgba(
        bytes: Arc<Vec<u8>>,
        page_index: usize,
        max_w: u32,
        max_h: u32,
    ) -> Result<RenderedPdfPage, PdfError> {
        render_pdf_page_rgba(bytes, page_index, max_w, max_h)
    }
    /// Exports all pages (or selected 0-based `pages`) of the active notebook as PDF bytes without touching the filesystem.
    pub fn export_pdf_bytes(&mut self) -> Result<Vec<u8>, EngineError> {
        self.export_pdf_bytes_for_pages(None)
    }

    /// Exports either all pages (`None`) or the selected 0-based `pages` as PDF bytes.
    pub fn export_pdf_bytes_for_pages(
        &mut self,
        pages: Option<&[usize]>,
    ) -> Result<Vec<u8>, EngineError> {
        let (pdf, page_count) = if let Some(indices) = pages {
            let ordered =
                validate_page_selection_ordered("export.pdf", indices, self.doc.page_count())?;
            (
                export_notebook_pdf_pages(&self.doc, &ordered),
                ordered.len(),
            )
        } else {
            (export_notebook_pdf(&self.doc), self.doc.page_count())
        };
        self.last_export_status = Some(format!(
            "Prepared {page_count}-page PDF ({} bytes).",
            pdf.len()
        ));
        Ok(pdf)
    }

    /// Exports `page` (or the active page) as SVG bytes without touching the filesystem.
    pub fn export_svg_bytes(&mut self, page: Option<usize>) -> Result<Vec<u8>, EngineError> {
        let page_doc = resolve_page_snapshot(self, "export.svg", page)?;
        let svg = render_document_svg(&page_doc);
        self.last_export_status = Some(format!("Prepared SVG ({} bytes).", svg.len()));
        Ok(svg.into_bytes())
    }

    /// Exports `page` (or the active page) as PNG bytes without touching the filesystem.
    pub fn export_png_bytes(&mut self, page: Option<usize>) -> Result<Vec<u8>, EngineError> {
        let page_doc = resolve_page_snapshot(self, "export.png", page)?;
        let surface = render_document(&page_doc, None);
        let png = surface
            .encode_png()
            .map_err(|err| invalid("export.png", format!("PNG encoding failed: {err}")))?;
        self.last_export_status = Some(format!("Prepared PNG ({} bytes).", png.len()));
        Ok(png)
    }

    /// Serializes the active notebook to binary `.dcraft` bytes without touching the filesystem.
    pub fn save_notebook_bytes(&mut self) -> Result<Vec<u8>, EngineError> {
        self.save_notebook_bytes_for_pages(None)
    }

    /// Serializes either all pages (`None`) or the selected 0-based `pages` (in order) to `.dcraft` bytes.
    pub fn save_notebook_bytes_for_pages(
        &mut self,
        pages: Option<&[usize]>,
    ) -> Result<Vec<u8>, EngineError> {
        let snapshot = build_notebook_subset_snapshot(&self.doc, "notebook.save", None, pages)?;
        let bytes = save_document_bytes(&snapshot)
            .map_err(|err| invalid("notebook.save", format!("save failed: {err}")))?;
        self.last_export_status = Some(format!(
            "Prepared \"{}\" ({} pages, {} bytes).",
            snapshot.title,
            snapshot.page_count(),
            bytes.len()
        ));
        Ok(bytes)
    }

    /// Loads a `.dcraft` (or legacy `.ncraft`) notebook directly from raw bytes (used by browser file pickers and drag-and-drop).
    pub fn load_notebook_bytes(
        &mut self,
        bytes: &[u8],
        source_name: Option<&str>,
        replace_active: bool,
    ) -> Result<Value, EngineError> {
        let doc = decode_notebook_bytes("notebook.load", bytes)?;
        Ok(insert_loaded_document(
            self,
            doc,
            source_name,
            replace_active,
            true,
        ))
    }

    /// Exports all active notebooks, trashed notebooks, folders, templates, and preferences
    /// into a single compressed `.dcraftpkg` package archive.
    pub fn export_package_bytes(&mut self) -> Result<Vec<u8>, EngineError> {
        self.sync_active_notebook_slot();
        let mut notebooks = Vec::new();
        for (idx, doc_arc) in self.notebooks.iter().enumerate() {
            if self
                .sync
                .slot_placeholder_blank
                .get(idx)
                .copied()
                .unwrap_or(false)
            {
                continue;
            }
            let mut d = (**doc_arc).clone();
            d.sync_active_to_pages();
            notebooks.push(d);
        }
        let trashed_notebooks = self
            .trashed_notebooks
            .iter()
            .map(|entry| PackagedTrashEntry {
                deleted_at_ms: entry.deleted_at_ms,
                document: (*entry.doc).clone(),
            })
            .collect();
        let source_name = if let Some(acct) = self
            .active_account_session()
            .filter(|_| self.is_viewing_cloud())
        {
            if acct.user.display_name.trim().is_empty() {
                acct.user.username.clone()
            } else {
                acct.user.display_name.clone()
            }
        } else {
            self.active_local_profile()
                .map_or_else(|| "Personal".to_string(), |p| p.nickname.clone())
        };
        let pkg = WorkspacePackage {
            exported_at_ms: drawcraft_sync_protocol::now_unix_ms(),
            source_name,
            prefs: self.prefs.clone(),
            notebooks,
            trashed_notebooks,
        };
        let bytes = save_package_bytes(&pkg)
            .map_err(|err| invalid("package.export", format!("package export failed: {err}")))?;
        self.last_export_status = Some(format!(
            "Exported complete package ({} notebooks, {} trashed, {} bytes).",
            pkg.notebooks.len(),
            pkg.trashed_notebooks.len(),
            bytes.len()
        ));
        Ok(bytes)
    }

    /// Restores notebooks, trashed items, folders, templates, and preferences from a
    /// complete `.dcraftpkg` archive.
    pub fn import_package_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<PackageImportSummary, EngineError> {
        let pkg = load_package_bytes(bytes)
            .map_err(|err| invalid("package.import", format!("invalid package archive: {err}")))?;
        self.sync_active_notebook_slot();
        self.cancel_live_stroke();
        self.cancel_live_eraser();

        let mut imported_notebooks = 0usize;
        let mut updated_notebooks = 0usize;
        let mut imported_trashed = 0usize;

        for mut doc in pkg.notebooks {
            let clean_id = drawcraft_doc::sanitize_notebook_id(&doc.id);
            doc.id = if clean_id.is_empty() {
                uuid::Uuid::new_v4().to_string()
            } else {
                clean_id
            };
            doc.title = sanitize_notebook_title(&doc.title);
            doc.sync_active_to_pages();
            doc.load_active_from_pages();
            if insert_or_update_packaged_notebook(self, doc) {
                updated_notebooks = updated_notebooks.saturating_add(1);
            } else {
                imported_notebooks = imported_notebooks.saturating_add(1);
            }
        }

        for mut entry in pkg.trashed_notebooks {
            let clean_id = drawcraft_doc::sanitize_notebook_id(&entry.document.id);
            entry.document.id = if clean_id.is_empty() {
                uuid::Uuid::new_v4().to_string()
            } else {
                clean_id
            };
            let already_exists = self.notebooks.iter().any(|d| d.id == entry.document.id)
                || self
                    .trashed_notebooks
                    .iter()
                    .any(|t| t.doc.id == entry.document.id);
            if !already_exists
                && self
                    .add_trash_entry_persisted(entry.document, entry.deleted_at_ms)
                    .is_ok()
            {
                imported_trashed = imported_trashed.saturating_add(1);
            }
        }

        crate::sync_state::merge_studio_prefs(&mut self.prefs, pkg.prefs, true);
        self.apply_pen_size_preferences();
        let _ = self.save_prefs();
        if !self.notebooks.is_empty() {
            let idx = self.active_notebook.min(self.notebooks.len() - 1);
            self.active_notebook = idx;
            self.doc = Arc::clone(&self.notebooks[idx]);
        }
        self.history.clear();
        self.sync_reconcile_slots();
        self.sync_mark_prefs_dirty();
        let _ = self.save_library();
        self.sync_persist_manifest();
        self.save_current_web_bundle();
        self.bump_revision();

        self.last_export_status = Some(format!(
            "Imported complete package ({imported_notebooks} new, {updated_notebooks} updated, {imported_trashed} trashed).",
        ));
        Ok(PackageImportSummary {
            imported_notebooks,
            updated_notebooks,
            imported_trashed,
            source_name: pkg.source_name,
        })
    }
}

/// Summary returned after importing a `.dcraftpkg` complete package archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageImportSummary {
    pub imported_notebooks: usize,
    pub updated_notebooks: usize,
    pub imported_trashed: usize,
    pub source_name: String,
}

fn insert_or_update_packaged_notebook(session: &mut Session, doc: drawcraft_doc::Document) -> bool {
    let id = doc.id.clone();
    if let Some(idx) = session.find_slot_by_id(&id) {
        session.notebooks[idx] = Arc::new(doc);
        session.library.mark_dirty(idx, session.notebooks.len());
        session.sync_mark_slot_dirty(idx);
        return true;
    }
    if let Some(p_idx) = session.sync.slot_placeholder_blank.iter().position(|&b| b) {
        session.notebooks[p_idx] = Arc::new(doc);
        session.library.mark_dirty(p_idx, session.notebooks.len());
        if p_idx < session.sync.slot_ids.len() {
            session.sync.slot_ids[p_idx] = id;
        }
        if p_idx < session.sync.slot_dirty_hash.len() {
            session.sync.slot_dirty_hash[p_idx] = true;
        }
        session.sync.slot_placeholder_blank[p_idx] = false;
        session.active_notebook = p_idx;
        return false;
    }
    if session.notebooks.len() < MAX_NOTEBOOKS {
        session.notebooks.push(Arc::new(doc));
        let count = session.notebooks.len();
        session.library.push_slot(None, true, count);
        session.sync.slot_ids.push(id);
        session.sync.slot_dirty_hash.push(true);
        session.sync.slot_placeholder_blank.push(false);
        session.active_notebook = count.saturating_sub(1);
    }
    false
}

fn build_notebook_subset_snapshot(
    doc: &Document,
    id: &str,
    page: Option<usize>,
    pages: Option<&[usize]>,
) -> Result<Document, EngineError> {
    if let Some(indices) = pages {
        let resolved_pages = doc.resolved_pages();
        let ordered = validate_page_selection_ordered(id, indices, resolved_pages.len())?;
        let mut snapshot = doc
            .document_for_page(ordered[0])
            .ok_or_else(|| invalid(id, "page index out of bounds"))?;
        for &idx in &ordered[1..] {
            snapshot
                .append_cloned_page(&resolved_pages[idx], doc)
                .map_err(|reason| invalid(id, reason))?;
        }
        snapshot.current_page = 0;
        snapshot.load_active_from_pages();
        return Ok(snapshot);
    }
    if let Some(idx) = page {
        return doc
            .document_for_page(idx)
            .ok_or_else(|| invalid(id, "page index out of bounds"));
    }
    let mut snapshot = doc.clone();
    snapshot.sync_active_to_pages();
    Ok(snapshot)
}

fn resolve_page_snapshot(
    session: &Session,
    id: &str,
    page_opt: Option<usize>,
) -> Result<drawcraft_doc::Document, EngineError> {
    let idx = page_opt.unwrap_or_else(|| session.doc.active_page_index());
    session
        .doc
        .document_for_page(idx)
        .ok_or_else(|| invalid(id, "page index out of bounds"))
}

fn cmd_export_png(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SaveOrExportParams = parse_optional_params(id, params)?;
    let page_doc = resolve_page_snapshot(session, id, p.page)?;
    let surface = render_document(&page_doc, None);
    let png = surface
        .encode_png()
        .map_err(|err| invalid(id, format!("PNG encoding failed: {err}")))?;
    if p.in_memory == Some(true) {
        session.last_export_status = Some(format!("Rendered PNG ({} bytes).", png.len()));
        return Ok(json!({ "bytes": png.len(), "pageCount": 1 }));
    }
    let written = write_export_bytes(id, p.path.as_deref(), &png)?;
    session.last_export_status = Some(format!("Exported PNG to `{written}`."));
    Ok(json!({ "path": written, "bytes": png.len(), "pageCount": 1 }))
}

fn cmd_export_svg(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SaveOrExportParams = parse_optional_params(id, params)?;
    let page_doc = resolve_page_snapshot(session, id, p.page)?;
    let svg = render_document_svg(&page_doc);
    if p.in_memory == Some(true) {
        session.last_export_status = Some(format!("Rendered SVG ({} bytes).", svg.len()));
        return Ok(json!({ "bytes": svg.len(), "svg": svg }));
    }
    let written = write_export_bytes(id, p.path.as_deref(), svg.as_bytes())?;
    session.last_export_status = Some(format!("Exported SVG to `{written}`."));
    Ok(json!({ "path": written, "bytes": svg.len() }))
}

fn cmd_export_pdf(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SaveOrExportParams = parse_optional_params(id, params)?;
    let (pdf, page_count) = match p.pages {
        Some(ref raw_pages) => {
            let ordered = validate_page_selection_ordered(id, raw_pages, session.doc.page_count())?;
            (
                export_notebook_pdf_pages(&session.doc, &ordered),
                ordered.len(),
            )
        }
        None => (export_notebook_pdf(&session.doc), session.doc.page_count()),
    };
    if p.in_memory == Some(true) {
        session.last_export_status = Some(format!(
            "Generated {page_count}-page PDF ({} bytes).",
            pdf.len()
        ));
        return Ok(json!({ "bytes": pdf.len(), "pageCount": page_count }));
    }
    let written = write_export_bytes(id, p.path.as_deref(), &pdf)?;
    session.last_export_status = Some(format!("Exported {page_count}-page PDF to `{written}`."));
    Ok(json!({ "path": written, "bytes": pdf.len(), "pageCount": page_count }))
}

fn cmd_select_tool(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SelectToolParams = parse_params(id, params)?;
    let tool = match p.tool.as_str() {
        "Select" | "select" => ToolKind::Select,
        "FountainPen" | "fountain_pen" => ToolKind::Pen(PenKind::FountainPen),
        "BallPen" | "ball_pen" => ToolKind::Pen(PenKind::BallPen),
        "BrushPen" | "brush_pen" => ToolKind::Pen(PenKind::BrushPen),
        "Highlighter" | "highlighter" => ToolKind::Pen(PenKind::Highlighter),
        "Pencil" | "pencil" => ToolKind::Pen(PenKind::Pencil),
        "Eraser" | "eraser" => ToolKind::Eraser,
        "LaserPointer" | "laser_pointer" => ToolKind::LaserPointer,
        other => return Err(invalid(id, format!("unknown tool `{other}`"))),
    };
    session.set_active_tool(tool);
    Ok(json!({ "tool": session.active_tool }))
}

fn apply_pen_patches(
    pen: &mut PenSettings,
    p: &SetPenSettingsParams,
    id: &str,
) -> Result<(), EngineError> {
    if let Some(c) = p.color {
        pen.color = c;
    }
    if let Some(ref hex) = p.color_hex {
        pen.color = Color::from_hex(hex)
            .ok_or_else(|| invalid(id, format!("invalid hex color `{hex}`")))?;
    }
    if let Some(v) = p.size {
        pen.size = v;
    }
    if let Some(v) = p.opacity {
        pen.opacity = v;
    }
    if let Some(v) = p.hardness {
        pen.hardness = v;
    }
    if let Some(v) = p.pressure_enabled {
        pen.pressure_enabled = v;
    }
    if let Some(v) = p.pressure_size_min {
        pen.pressure_size_min = v;
    }
    if let Some(v) = p.pressure_opacity_min {
        pen.pressure_opacity_min = v;
    }
    if let Some(v) = p.velocity_pressure_simulation {
        pen.velocity_pressure_simulation = v;
    }
    if let Some(v) = p.tilt_sensitivity {
        pen.tilt_sensitivity = v;
    }
    if let Some(v) = p.nib_flatness {
        pen.nib_flatness = v;
    }
    if let Some(v) = p.nib_angle_deg {
        pen.nib_angle_deg = v;
    }
    if let Some(v) = p.start_taper_px {
        pen.start_taper_px = v;
    }
    if let Some(v) = p.end_taper_px {
        pen.end_taper_px = v;
    }
    Ok(())
}

fn cmd_set_pen_settings(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetPenSettingsParams = parse_params(id, params)?;
    let kind = p.kind.unwrap_or_else(|| session.active_pen_kind());
    let mut pen = match p.settings {
        Some(ref s) => {
            let mut cloned = s.clone();
            cloned.kind = kind;
            cloned
        }
        None => session.pen(kind).clone(),
    };
    apply_pen_patches(&mut pen, &p, id)?;
    let clean = pen.sanitized();
    *session.pen_mut(kind) = clean.clone();
    if (p.size.is_some() || p.settings.is_some()) && p.persist_size.unwrap_or(true) {
        remember_pen_size(session, kind, clean.size);
    }
    Ok(json!({ "kind": kind, "settings": clean }))
}

fn remember_pen_size(session: &mut Session, kind: PenKind, size: f32) {
    let Some(saved_size) = session.prefs.pen_sizes.get_mut(kind.index()) else {
        return;
    };
    if *saved_size == size {
        return;
    }
    *saved_size = size;
    session.sync_mark_prefs_dirty();
    let _ = session.save_prefs();
}

fn apply_buffer_patches(sm: &mut SmoothingConfig, p: &SetSmoothingParams) {
    if let Some(v) = p.buffer_averaging {
        sm.buffer_averaging = v;
    }
    if let Some(v) = p.buffer_preprocessor {
        sm.buffer_preprocessor = v;
    }
    if let Some(v) = p.buffer_size {
        sm.buffer_size = v;
    }
    if let Some(v) = p.buffer_sigma {
        sm.buffer_sigma = v;
    }
    if let Some(v) = p.buffer_deadzone_radius {
        sm.buffer_deadzone_radius = v;
    }
    if let Some(v) = p.buffer_cusp_detection {
        sm.buffer_cusp_detection = v;
    }
    if let Some(v) = p.buffer_drag {
        sm.buffer_drag = v;
    }
    if let Some(v) = p.buffer_mass {
        sm.buffer_mass = v;
    }
    if let Some(v) = p.buffer_finalize_stroke {
        sm.buffer_finalize_stroke = v;
    }
}

fn cmd_set_smoothing(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetSmoothingParams = parse_params(id, params)?;
    let kind = p.kind.unwrap_or_else(|| session.active_pen_kind());
    let mut sm = if let Some(preset) = p.preset {
        preset.to_smoothing_config()
    } else if let Some(smoothing) = &p.smoothing {
        smoothing.clone()
    } else {
        session.pen(kind).smoothing.clone()
    };
    if let Some(m) = p.stabilizer_mode {
        sm.stabilizer_mode = m;
    }
    if let Some(v) = p.stabilizer_strength {
        sm.stabilizer_strength = v;
    }
    if let Some(v) = p.catmull_rom_subdivision {
        sm.catmull_rom_subdivision = v;
    }
    if let Some(v) = p.spline_tension {
        sm.spline_tension = v;
    }
    if let Some(v) = p.adjust_for_zoom {
        sm.adjust_for_zoom = v;
    }
    if let Some(v) = p.catch_up {
        sm.catch_up = v;
    }
    if let Some(v) = p.catch_up_on_end {
        sm.catch_up_on_end = v;
    }
    if let Some(v) = p.streamline_amount {
        sm.streamline_amount = v;
    }
    if let Some(v) = p.prediction_ms {
        sm.prediction_ms = v;
    }
    apply_buffer_patches(&mut sm, &p);
    let clean = sm.sanitized();
    session.pen_mut(kind).smoothing = clean.clone();
    Ok(json!({ "kind": kind, "preset": clean.preset(), "smoothing": clean }))
}

fn cmd_set_pressure_curve(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetPressureCurveParams = parse_params(id, params)?;
    let kind = p.kind.unwrap_or_else(|| session.active_pen_kind());
    let curve = if let Some(preset) = p.preset {
        PressureCurve::from_preset(preset)
    } else if let Some(pts) = p.points {
        if pts.len() < 2 {
            return Err(invalid(id, "pressure curve needs at least 2 points"));
        }
        PressureCurve::new(pts)
    } else {
        return Err(invalid(id, "provide `preset` or `points`"));
    };
    session.pen_mut(kind).pressure_curve = curve.clone();
    Ok(json!({ "kind": kind, "pressureCurve": curve }))
}

fn cmd_apply_pen_preset(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: ApplyPenPresetParams = parse_params(id, params)?;
    let kind = p.preset.kind();
    let preserve = p.preserve_color.unwrap_or(true);
    let updated = if preserve {
        p.preset.apply_preserving_color(session.pen(kind).color)
    } else {
        p.preset.settings()
    }
    .sanitized();
    *session.pen_mut(kind) = updated.clone();
    remember_pen_size(session, kind, updated.size);
    session.set_active_tool(ToolKind::Pen(kind));
    Ok(json!({ "kind": kind, "preset": p.preset, "settings": updated }))
}

fn cmd_set_eraser(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SetEraserParams = parse_params(id, params)?;
    if let Some(m) = p.mode {
        session.eraser_mode = m;
    }
    if let Some(s) = p.size {
        if !s.is_finite() || s <= 0.0 {
            return Err(invalid(id, "eraser size must be finite and positive"));
        }
        session.eraser_size = s.clamp(2.0, 160.0);
    }
    if let Some(ho) = p.highlighter_only {
        session.eraser_highlighter_only = ho;
    }
    Ok(json!({
        "mode": session.eraser_mode,
        "size": session.eraser_size,
        "highlighter_only": session.eraser_highlighter_only,
    }))
}

fn cmd_set_highlighter(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetHighlighterParams = parse_params(id, params)?;
    if let Some(m) = p.mode {
        session.highlighter_mode = m;
    }
    Ok(json!({ "mode": session.highlighter_mode }))
}

fn parse_finite_opaque_color(
    cmd_id: &str,
    color: Option<Color>,
    color_hex: Option<&str>,
) -> Result<Color, EngineError> {
    let raw_color = match (color, color_hex) {
        (_, Some(hex)) => Color::from_hex(hex)
            .ok_or_else(|| invalid(cmd_id, format!("invalid hex color `{hex}`")))?,
        (Some(c), None) => c,
        (None, None) => return Err(invalid(cmd_id, "provide `color` or `color_hex`")),
    };
    if !(raw_color.r.is_finite()
        && raw_color.g.is_finite()
        && raw_color.b.is_finite()
        && raw_color.a.is_finite())
    {
        return Err(invalid(cmd_id, "color components must be finite"));
    }
    Ok(raw_color.sanitized().with_alpha(1.0))
}

fn allocate_unique_id(existing: &[&str], preferred: Option<&str>, prefix: &str) -> String {
    if let Some(raw) = preferred {
        let slug = sanitize_preset_id(raw);
        if !slug.is_empty() && !existing.contains(&slug.as_str()) {
            return slug;
        }
    }
    let mut idx = existing.len().saturating_add(1);
    loop {
        let candidate = format!("{prefix}_{idx}");
        if !existing.contains(&candidate.as_str()) {
            return candidate;
        }
        idx = idx.saturating_add(1);
    }
}

fn sanitize_color_vec(
    cmd_id: &str,
    raw: Vec<Color>,
    field_name: &str,
) -> Result<Vec<Color>, EngineError> {
    if raw.is_empty() {
        return Err(invalid(cmd_id, format!("{field_name} cannot be empty")));
    }
    let mut out = Vec::with_capacity(raw.len().min(MAX_PALETTE_COLORS));
    for c in raw.into_iter().take(MAX_PALETTE_COLORS) {
        if !(c.r.is_finite() && c.g.is_finite() && c.b.is_finite() && c.a.is_finite()) {
            return Err(invalid(cmd_id, "color components must be finite"));
        }
        out.push(c.sanitized().with_alpha(1.0));
    }
    Ok(out)
}

fn apply_target_pen_color(session: &mut Session, target: PaletteTarget, color: Color) {
    let active_kind = session.active_pen_kind();
    let active_matches_target = PaletteTarget::for_pen(active_kind) == target;
    if active_matches_target {
        session.pen_mut(active_kind).color = color;
    } else if target == PaletteTarget::Highlighter {
        session.pen_mut(PenKind::Highlighter).color = color;
    } else {
        session.pen_mut(PenKind::FountainPen).color = color;
    }
}

fn cmd_palette_set_wildcard(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetWildcardColorParams = parse_params(id, params)?;
    let target = p
        .target
        .unwrap_or_else(|| PaletteTarget::for_pen(session.active_pen_kind()));
    let clean_color = parse_finite_opaque_color(id, p.color, p.color_hex.as_deref())?;
    session.prefs.set_wildcard_color(target, clean_color);

    if p.apply_to_pen.unwrap_or(true) {
        apply_target_pen_color(session, target, clean_color);
    }

    let _ = session.save_prefs();
    Ok(json!({
        "target": target,
        "color": clean_color,
    }))
}

fn cmd_palette_set_color(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetPaletteColorParams = parse_params(id, params)?;
    let target = p
        .target
        .unwrap_or_else(|| PaletteTarget::for_pen(session.active_pen_kind()));
    let clean_color = parse_finite_opaque_color(id, p.color, p.color_hex.as_deref())?;

    let target_preset_id = p
        .preset_id
        .as_deref()
        .map(sanitize_preset_id)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| session.prefs.active_palette_id.clone());

    let slot_index = {
        let preset = session
            .prefs
            .palette_by_id_mut(&target_preset_id)
            .ok_or_else(|| invalid(id, format!("unknown color palette `{target_preset_id}`")))?;
        if preset.built_in {
            return Err(invalid(
                id,
                "built-in default color palettes are read-only; duplicate or create a custom palette to edit swatches",
            ));
        }
        let palette = preset.colors_mut(target);
        if let Some(idx) = p.index {
            if idx >= palette.len() {
                return Err(invalid(id, "palette swatch index out of bounds"));
            }
            palette[idx] = clean_color;
            idx
        } else {
            if palette.len() >= MAX_PALETTE_COLORS {
                return Err(invalid(id, "palette swatch limit reached"));
            }
            palette.push(clean_color);
            palette.len().saturating_sub(1)
        }
    };
    session.prefs.sync_active_palettes();

    if p.apply_to_pen.unwrap_or(true) {
        apply_target_pen_color(session, target, clean_color);
    }

    let count = session
        .prefs
        .palette_by_id(&target_preset_id)
        .map_or(0, |pal| pal.colors(target).len());
    let _ = session.save_prefs();
    Ok(json!({
        "preset_id": target_preset_id,
        "target": target,
        "index": slot_index,
        "color": clean_color,
        "count": count
    }))
}

fn cmd_palette_remove_color(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: RemovePaletteColorParams = parse_params(id, params)?;
    let target = p
        .target
        .unwrap_or_else(|| PaletteTarget::for_pen(session.active_pen_kind()));
    let target_preset_id = p
        .preset_id
        .as_deref()
        .map(sanitize_preset_id)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| session.prefs.active_palette_id.clone());

    let (removed, remaining) = {
        let preset = session
            .prefs
            .palette_by_id_mut(&target_preset_id)
            .ok_or_else(|| invalid(id, format!("unknown color palette `{target_preset_id}`")))?;
        if preset.built_in {
            return Err(invalid(
                id,
                "built-in default color palettes are read-only; duplicate or create a custom palette to edit swatches",
            ));
        }
        let palette = preset.colors_mut(target);
        if palette.len() <= 1 {
            return Err(invalid(
                id,
                "cannot remove the last remaining palette color",
            ));
        }
        if p.index >= palette.len() {
            return Err(invalid(id, "palette swatch index out of bounds"));
        }
        let removed = palette.remove(p.index);
        (removed, palette.len())
    };
    session.prefs.sync_active_palettes();

    let _ = session.save_prefs();
    Ok(json!({
        "preset_id": target_preset_id,
        "target": target,
        "removed": removed,
        "count": remaining
    }))
}

fn cmd_palette_reset(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: ResetPaletteParams = parse_optional_params(id, params)?;
    let scope = p.target.as_deref().unwrap_or("all");
    let target_id =
        p.id.as_deref()
            .map(sanitize_preset_id)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| session.prefs.active_palette_id.clone());

    let factory = ColorPalettePreset::builtin_by_id(&target_id)
        .unwrap_or_else(ColorPalettePreset::builtin_classic);
    let preset = session
        .prefs
        .palette_by_id_mut(&target_id)
        .ok_or_else(|| invalid(id, format!("unknown color palette `{target_id}`")))?;

    match scope {
        "ink" | "Ink" => {
            preset.ink_colors = factory.ink_colors;
        }
        "highlighter" | "Highlighter" => {
            preset.highlighter_colors = factory.highlighter_colors;
        }
        "all" | "All" => {
            preset.ink_colors = factory.ink_colors;
            preset.highlighter_colors = factory.highlighter_colors;
        }
        other => return Err(invalid(id, format!("unknown palette target `{other}`"))),
    }
    session.prefs.sync_active_palettes();
    let _ = session.save_prefs();
    Ok(json!({
        "preset_id": target_id,
        "inkCount": session.prefs.ink_palette.len(),
        "highlighterCount": session.prefs.highlighter_palette.len()
    }))
}

fn cmd_palette_select_preset(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SelectPalettePresetParams = parse_params(id, params)?;
    let req_id = sanitize_preset_id(&p.id);
    if req_id.is_empty() || session.prefs.palette_by_id(&req_id).is_none() {
        return Err(invalid(id, format!("unknown color palette `{}`", p.id)));
    }
    session.prefs.active_palette_id = req_id.clone();
    session.prefs.sync_active_palettes();
    let _ = session.save_prefs();
    Ok(json!({
        "active_id": req_id,
        "preset": session.prefs.active_palette(),
    }))
}

fn cmd_palette_save_preset(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SavePalettePresetParams = parse_params(id, params)?;
    let has_non_control = p.name.trim().chars().any(|ch| !ch.is_control());
    if !has_non_control {
        return Err(invalid(id, "palette name cannot be empty"));
    }
    let clean_name = sanitize_palette_name(&p.name);

    let custom_ink = p
        .ink_colors
        .map(|v| sanitize_color_vec(id, v, "ink_colors"))
        .transpose()?;
    let custom_hi = p
        .highlighter_colors
        .map(|v| sanitize_color_vec(id, v, "highlighter_colors"))
        .transpose()?;

    let req_id =
        p.id.as_deref()
            .map(sanitize_preset_id)
            .filter(|s| !s.is_empty());

    let preset_id = if let Some(req) = req_id
        && let Some(pos) = session
            .prefs
            .palette_presets
            .iter()
            .position(|x| x.id == req)
    {
        let existing = &mut session.prefs.palette_presets[pos];
        if existing.built_in {
            return Err(invalid(id, "built-in default color palettes are read-only"));
        }
        existing.name = clean_name;
        if let Some(ink) = custom_ink {
            existing.ink_colors = ink;
        }
        if let Some(hi) = custom_hi {
            existing.highlighter_colors = hi;
        }
        existing.id.clone()
    } else {
        if session.prefs.palette_presets.len() >= MAX_PALETTE_PRESETS {
            return Err(invalid(id, "color palette preset limit reached"));
        }
        let seed_color = match (p.seed_color, p.seed_color_hex.as_deref()) {
            (None, None) => None,
            (c, hex) => Some(parse_finite_opaque_color(id, c, hex)?),
        };
        let copy_source = if let Some(raw_source) = p.copy_from_id.as_deref() {
            let source_id = sanitize_preset_id(raw_source);
            let found = session
                .prefs
                .palette_by_id(&source_id)
                .cloned()
                .ok_or_else(|| invalid(id, format!("unknown color palette `{raw_source}`")))?;
            Some(found)
        } else if p.copy_from_active.unwrap_or(false) {
            Some(session.prefs.active_palette().cloned().unwrap_or_default())
        } else {
            None
        };
        let active_target = PaletteTarget::for_pen(session.active_pen_kind());
        let active_pal = session.prefs.active_palette().cloned().unwrap_or_default();

        let ink_colors = if let Some(ink) = custom_ink {
            ink
        } else if let Some(ref src) = copy_source {
            src.ink_colors.clone()
        } else if active_target == PaletteTarget::Ink {
            vec![
                seed_color
                    .unwrap_or_else(|| session.active_pen().color.sanitized().with_alpha(1.0)),
            ]
        } else {
            vec![
                active_pal
                    .ink_colors
                    .first()
                    .copied()
                    .unwrap_or(Color::BLACK),
            ]
        };

        let highlighter_colors = if let Some(hi) = custom_hi {
            hi
        } else if let Some(ref src) = copy_source {
            src.highlighter_colors.clone()
        } else if active_target == PaletteTarget::Highlighter {
            vec![seed_color.unwrap_or_else(|| {
                session
                    .pen(PenKind::Highlighter)
                    .color
                    .sanitized()
                    .with_alpha(1.0)
            })]
        } else {
            vec![
                active_pal
                    .highlighter_colors
                    .first()
                    .copied()
                    .unwrap_or_else(|| {
                        session
                            .pen(PenKind::Highlighter)
                            .color
                            .sanitized()
                            .with_alpha(1.0)
                    }),
            ]
        };

        let existing_ids: Vec<&str> = session
            .prefs
            .palette_presets
            .iter()
            .map(|x| x.id.as_str())
            .collect();
        let new_id = allocate_unique_id(&existing_ids, None, "palette");
        session.prefs.palette_presets.push(ColorPalettePreset {
            id: new_id.clone(),
            name: clean_name,
            built_in: false,
            ink_colors,
            highlighter_colors,
        });
        new_id
    };

    if p.select.unwrap_or(true) {
        session.prefs.active_palette_id = preset_id.clone();
    }
    session.prefs.sync_active_palettes();

    let saved = session
        .prefs
        .palette_by_id(&preset_id)
        .cloned()
        .ok_or_else(|| invalid(id, "failed to store color palette preset"))?;
    let _ = session.save_prefs();
    Ok(json!({
        "id": saved.id,
        "preset": saved,
        "active_id": session.prefs.active_palette_id,
        "count": session.prefs.palette_presets.len(),
    }))
}

fn cmd_palette_delete_preset(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DeletePalettePresetParams = parse_params(id, params)?;
    let req_id = sanitize_preset_id(&p.id);
    let Some(pos) = session
        .prefs
        .palette_presets
        .iter()
        .position(|x| x.id == req_id)
    else {
        return Err(invalid(id, format!("unknown color palette `{}`", p.id)));
    };
    if session.prefs.palette_presets[pos].built_in {
        return Err(invalid(
            id,
            "built-in default color palettes cannot be deleted",
        ));
    }

    session.prefs.palette_presets.remove(pos);
    if session.prefs.active_palette_id == req_id {
        let fallback_pos = pos.saturating_sub(1);
        session.prefs.active_palette_id =
            session.prefs.palette_presets.get(fallback_pos).map_or_else(
                || BUILTIN_PALETTE_CLASSIC_ID.to_string(),
                |pal| pal.id.clone(),
            );
    }
    session.prefs.sync_active_palettes();

    let _ = session.save_prefs();
    Ok(json!({
        "deleted_id": req_id,
        "active_id": session.prefs.active_palette_id,
        "count": session.prefs.palette_presets.len(),
    }))
}

fn allocate_unique_preset_id(existing: &[CustomPenPreset], preferred: Option<&str>) -> String {
    let ids: Vec<&str> = existing.iter().map(|p| p.id.as_str()).collect();
    allocate_unique_id(&ids, preferred, "custom")
}

fn cmd_preset_save(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SaveCustomPresetParams = parse_params(id, params)?;
    let clean_name = sanitize_preset_name(&p.name);
    let clean_desc = p
        .description
        .as_deref()
        .map(sanitize_preset_description)
        .unwrap_or_default();
    let clean_settings = p.settings.sanitized();
    let include_color = p.include_color.unwrap_or(false);

    let requested_id =
        p.id.as_deref()
            .map(sanitize_preset_id)
            .filter(|s| !s.is_empty());

    let existing_pos = requested_id.as_deref().and_then(|req| {
        session
            .prefs
            .custom_presets
            .iter()
            .position(|x| x.id == req)
    });

    let preset_id = if let Some(pos) = existing_pos {
        let existing_id = session.prefs.custom_presets[pos].id.clone();
        session.prefs.custom_presets[pos] = CustomPenPreset {
            id: existing_id.clone(),
            name: clean_name,
            description: clean_desc,
            settings: clean_settings.clone(),
            include_color,
        };
        existing_id
    } else {
        if session.prefs.custom_presets.len() >= MAX_CUSTOM_PRESETS {
            return Err(invalid(id, "custom preset limit reached"));
        }
        let new_id =
            allocate_unique_preset_id(&session.prefs.custom_presets, requested_id.as_deref());
        session.prefs.custom_presets.push(CustomPenPreset {
            id: new_id.clone(),
            name: clean_name,
            description: clean_desc,
            settings: clean_settings.clone(),
            include_color,
        });
        new_id
    };

    let saved = session
        .prefs
        .custom_presets
        .iter()
        .find(|x| x.id == preset_id)
        .cloned()
        .ok_or_else(|| invalid(id, "failed to store custom preset"))?;

    if p.apply_now.unwrap_or(false) {
        let kind = saved.settings.kind;
        let applied = saved.to_applied_settings(session.pen(kind).color, None);
        *session.pen_mut(kind) = applied.clone();
        remember_pen_size(session, kind, applied.size);
        session.set_active_tool(ToolKind::Pen(kind));
    }

    let _ = session.save_prefs();
    Ok(json!({
        "id": saved.id,
        "preset": saved,
        "count": session.prefs.custom_presets.len()
    }))
}

fn cmd_preset_apply_custom(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: ApplyCustomPresetParams = parse_params(id, params)?;
    let req_id = sanitize_preset_id(&p.id);
    if req_id.is_empty() {
        return Err(invalid(id, "preset `id` must not be empty"));
    }
    let preset = session
        .prefs
        .custom_presets
        .iter()
        .find(|x| x.id == req_id)
        .cloned()
        .ok_or_else(|| invalid(id, format!("unknown custom preset `{req_id}`")))?;

    let kind = preset.settings.kind;
    let applied = preset.to_applied_settings(session.pen(kind).color, p.preserve_color);
    *session.pen_mut(kind) = applied.clone();
    remember_pen_size(session, kind, applied.size);
    session.set_active_tool(ToolKind::Pen(kind));
    Ok(json!({
        "id": preset.id,
        "kind": kind,
        "settings": applied
    }))
}

fn cmd_preset_delete(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DeleteCustomPresetParams = parse_params(id, params)?;
    let req_id = sanitize_preset_id(&p.id);
    if req_id.is_empty() {
        return Err(invalid(id, "preset `id` must not be empty"));
    }
    let Some(pos) = session
        .prefs
        .custom_presets
        .iter()
        .position(|x| x.id == req_id)
    else {
        return Err(invalid(id, format!("unknown custom preset `{req_id}`")));
    };
    session.prefs.custom_presets.remove(pos);
    let _ = session.save_prefs();
    Ok(json!({
        "deleted": req_id,
        "count": session.prefs.custom_presets.len()
    }))
}

fn cmd_theme_set(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SetThemeParams = parse_params(id, params)?;
    if p.theme.is_none() && p.auto_contrast_with_theme.is_none() && p.ui_scale.is_none() {
        return Err(invalid(
            id,
            "requires at least one of `theme`, `auto_contrast_with_theme`, or `ui_scale`",
        ));
    }
    if let Some(raw_scale) = p.ui_scale
        && !raw_scale.is_finite()
    {
        return Err(invalid(id, "ui_scale must be a finite number"));
    }
    if let Some(theme) = p.theme {
        session.prefs.theme = theme;
    }
    if let Some(auto_contrast) = p.auto_contrast_with_theme {
        session.prefs.auto_contrast_with_theme = auto_contrast;
    }
    if let Some(raw_scale) = p.ui_scale {
        session.prefs.ui_scale = drawcraft_doc::UserStudioConfig::sanitize_ui_scale(raw_scale);
    }
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "theme": session.prefs.theme,
        "auto_contrast_with_theme": session.prefs.auto_contrast_with_theme,
        "ui_scale": session.prefs.ui_scale,
    }))
}

#[derive(Deserialize)]
struct SetStylusButtonsParams {
    barrel_button: Option<StylusButtonAction>,
    secondary_barrel_button: Option<StylusButtonAction>,
    eraser_end: Option<StylusButtonAction>,
    #[serde(default)]
    reset: bool,
}

fn cmd_stylus_set_buttons(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetStylusButtonsParams = parse_params(id, params)?;
    if !p.reset
        && p.barrel_button.is_none()
        && p.secondary_barrel_button.is_none()
        && p.eraser_end.is_none()
    {
        return Err(invalid(
            id,
            "requires `reset: true` or at least one of `barrel_button`, `secondary_barrel_button`, or `eraser_end`",
        ));
    }
    if let Some(eraser_action) = p.eraser_end
        && !eraser_action.supports_eraser_end()
    {
        return Err(invalid(
            id,
            "eraser_end only supports hold actions (`EraserHold`, `LassoSelectHold`, `PanCanvasHold`, `HighlighterHold`) or `None`",
        ));
    }
    if p.reset {
        session.prefs.stylus_buttons = StylusButtonConfig::default();
    }
    if let Some(action) = p.barrel_button {
        session.prefs.stylus_buttons.barrel_button = action;
    }
    if let Some(action) = p.secondary_barrel_button {
        session.prefs.stylus_buttons.secondary_barrel_button = action;
    }
    if let Some(action) = p.eraser_end {
        session.prefs.stylus_buttons.eraser_end = action;
    }
    session.prefs.stylus_buttons = session.prefs.stylus_buttons.sanitized();
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "barrel_button": session.prefs.stylus_buttons.barrel_button,
        "secondary_barrel_button": session.prefs.stylus_buttons.secondary_barrel_button,
        "eraser_end": session.prefs.stylus_buttons.eraser_end,
    }))
}

#[derive(Deserialize)]
struct SetServerUrlParams {
    url: String,
}

fn cmd_sync_status(session: &Session) -> Result<Value, EngineError> {
    let summary = session.sync_status_summary();
    Ok(json!({
        "signedIn": summary.signed_in,
        "viewingCloud": summary.viewing_cloud,
        "serverUrl": summary.server_url,
        "deviceName": summary.device_name,
        "username": summary.username,
        "email": summary.email,
        "displayName": summary.display_name,
        "activeLocalProfileId": summary.active_local_profile_id,
        "activeLocalProfileName": summary.active_local_profile_name,
        "pendingChanges": summary.pending_changes,
        "cloudPendingChanges": session.cloud_pending_changes_count(),
        "lastSyncAtMs": summary.last_sync_at_ms,
        "lastError": summary.last_error,
    }))
}

fn cmd_sync_set_server_url(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SetServerUrlParams = parse_params(id, params)?;
    let normalized = session
        .set_server_url_preference(&p.url)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({ "serverUrl": normalized }))
}

fn cmd_sync_sign_out(session: &mut Session) -> Result<Value, EngineError> {
    session.sign_out_to_local_library();
    let summary = session.sync_status_summary();
    Ok(json!({
        "signedIn": summary.signed_in,
        "notebookCount": session.notebooks.len(),
    }))
}

#[derive(Deserialize)]
struct TransferNotebookParams {
    #[serde(default, alias = "sourceProfileId")]
    source_profile_id: Option<String>,
    #[serde(alias = "notebookId")]
    notebook_id: String,
    #[serde(default, alias = "move", alias = "moveNotebook")]
    move_notebook: bool,
}

fn cmd_sync_transfer_notebook(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: TransferNotebookParams = parse_params(id, params)?;
    let source_id = p
        .source_profile_id
        .or_else(|| {
            session
                .active_local_profile()
                .map(|prof| prof.local_profile_id.clone())
        })
        .ok_or_else(|| invalid(id, "no source local profile available"))?;
    let transferred_id = session
        .transfer_local_notebook_to_cloud(&source_id, &p.notebook_id, p.move_notebook)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({
        "notebookId": transferred_id,
        "moved": p.move_notebook,
    }))
}

#[derive(Deserialize)]
struct MigrateLocalAccountParams {
    #[serde(alias = "profileId")]
    profile_id: String,
}

fn cmd_sync_migrate_local_account(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: MigrateLocalAccountParams = parse_params(id, params)?;
    let summary = session
        .migrate_local_account_to_cloud(&p.profile_id)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({
        "sourceProfileId": summary.source_profile_id,
        "sourceNickname": summary.source_nickname,
        "targetAccountName": summary.target_account_name,
        "movedNotebooks": summary.moved_notebooks,
        "movedTrash": summary.moved_trash,
    }))
}

#[derive(Deserialize)]
struct FirstAccessNicknameParams {
    nickname: String,
}

fn cmd_profile_complete_first_access(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: FirstAccessNicknameParams = parse_params(id, params)?;
    let prof = session
        .complete_first_access_nickname(&p.nickname)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({
        "localProfileId": prof.local_profile_id,
        "nickname": prof.nickname,
        "needsInitialNickname": session.needs_initial_nickname(),
    }))
}

fn cmd_package_export(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SaveOrExportParams = parse_optional_params(id, params)?;
    let bytes = session.export_package_bytes()?;
    if p.in_memory == Some(true) {
        return Ok(json!({
            "bytes": bytes.len(),
            "notebooks": session.notebooks.len(),
            "trashed": session.trashed_notebooks.len(),
        }));
    }
    let written = write_export_bytes(id, p.path.as_deref(), &bytes)?;
    session.last_export_status = Some(format!("Saved complete package to `{written}`."));
    Ok(json!({
        "path": written,
        "bytes": bytes.len(),
        "notebooks": session.notebooks.len(),
        "trashed": session.trashed_notebooks.len(),
    }))
}

#[derive(Deserialize)]
struct ImportPackageParams {
    path: String,
}

fn cmd_package_import(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: ImportPackageParams = parse_params(id, params)?;
    let clean = sanitize_io_path(id, &p.path)?;
    let raw = std::fs::read(&clean)
        .map_err(|err| invalid(id, format!("failed to read package file: {err}")))?;
    let summary = session.import_package_bytes(&raw)?;
    Ok(json!({
        "importedNotebooks": summary.imported_notebooks,
        "updatedNotebooks": summary.updated_notebooks,
        "importedTrashed": summary.imported_trashed,
        "sourceName": summary.source_name,
    }))
}

#[derive(Deserialize)]
struct CreateProfileParams {
    nickname: String,
    #[serde(default, alias = "switchNow")]
    switch_now: bool,
}

fn cmd_profile_create(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: CreateProfileParams = parse_params(id, params)?;
    let created = session
        .create_local_profile(&p.nickname, p.switch_now)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({
        "localProfileId": created.local_profile_id,
        "nickname": created.nickname,
    }))
}

#[derive(Deserialize)]
struct RenameProfileParams {
    #[serde(alias = "profileId")]
    profile_id: String,
    nickname: String,
}

fn cmd_profile_rename(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: RenameProfileParams = parse_params(id, params)?;
    session
        .rename_local_profile(&p.profile_id, &p.nickname)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({ "profileId": p.profile_id }))
}

#[derive(Deserialize)]
struct SwitchWorkspaceParams {
    #[serde(default)]
    target: Option<String>,
    #[serde(default, alias = "profileId")]
    profile_id: Option<String>,
}

fn cmd_profile_switch(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SwitchWorkspaceParams = parse_params(id, params)?;
    let effective_target = match p.target.as_deref() {
        Some(t) => t,
        None if p.profile_id.is_some() => "local",
        None => return Err(invalid(id, "target must be `local` or `cloud`")),
    };
    match effective_target {
        "cloud" => session
            .switch_to_cloud_workspace()
            .map_err(|reason| invalid(id, reason))?,
        "local" => {
            let pid = p
                .profile_id
                .ok_or_else(|| invalid(id, "provide `profile_id` when target is `local`"))?;
            session
                .switch_to_local_profile(&pid)
                .map_err(|reason| invalid(id, reason))?;
        }
        _ => return Err(invalid(id, "target must be `local` or `cloud`")),
    }
    Ok(json!({
        "viewingCloud": session.is_viewing_cloud(),
        "notebookCount": session.notebooks.len(),
    }))
}

#[derive(Deserialize)]
struct DeleteProfileParams {
    #[serde(alias = "profileId")]
    profile_id: String,
}

fn cmd_profile_delete(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DeleteProfileParams = parse_params(id, params)?;
    session
        .delete_local_profile(&p.profile_id)
        .map_err(|reason| invalid(id, reason))?;
    Ok(json!({ "deletedProfileId": p.profile_id }))
}

#[derive(Deserialize)]
struct RecognizeShapeParams {
    stroke_id: Option<StrokeId>,
    #[serde(default)]
    samples: Option<Vec<StrokeSample>>,
    color: Option<Color>,
    width: Option<f32>,
    include_line: Option<bool>,
    replace_stroke: Option<bool>,
}

fn cmd_stroke_recognize_shape(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: RecognizeShapeParams = parse_params(id, params)?;
    if let Some(ref raw_samples) = p.samples
        && raw_samples.len() > MAX_COMMAND_POINTS
    {
        return Err(invalid(id, "samples exceed maximum point count"));
    }
    if p.width.is_some_and(|w| !w.is_finite() || w <= 0.0) {
        return Err(invalid(id, "width must be finite and positive"));
    }

    let (samples, fallback_color, fallback_width, found_stroke_id) = if let Some(sid) = p.stroke_id
    {
        let Some(stroke) = session.doc.strokes.iter().find(|s| s.id == sid) else {
            return Err(invalid(id, "stroke does not exist on the active page"));
        };
        (
            stroke.smoothed_points.as_ref().clone(),
            stroke.pen.color,
            stroke.pen.size,
            Some(sid),
        )
    } else if let Some(s) = p.samples {
        (
            s.into_iter().map(|pt| pt.sanitized()).collect(),
            session.active_pen().color,
            session.active_pen().size,
            None,
        )
    } else {
        return Err(invalid(id, "requires either `stroke_id` or `samples`"));
    };

    let color = p.color.unwrap_or(fallback_color).sanitized();
    let width = p.width.unwrap_or(fallback_width);
    let include_line = p.include_line.unwrap_or(false);
    let Some(shape) = recognize_drawn_shape(&samples, color, width, include_line) else {
        return Ok(json!({ "recognized": false }));
    };

    if p.replace_stroke.unwrap_or(true)
        && let Some(sid) = found_stroke_id
    {
        if session.doc.shapes.len() >= MAX_PAGE_SHAPES {
            return Err(invalid(id, "active page shape limit reached"));
        }
        let before = Arc::clone(&session.doc);
        let mut next = (*session.doc).clone();
        next.strokes.retain(|s| s.id != sid);
        let shape_index = next.shapes.len();
        next.shapes.push(Arc::new(shape.clone()));
        next.sync_active_to_pages();
        session.history.record("Recognize Shape", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
        return Ok(json!({
            "recognized": true,
            "shape": shape,
            "shape_index": shape_index,
        }));
    }

    Ok(json!({
        "recognized": true,
        "shape": shape,
    }))
}

#[derive(Deserialize)]
struct ToggleCheckboxParams {
    index: usize,
    paragraph_index: usize,
}

fn cmd_text_toggle_checkbox(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: ToggleCheckboxParams = parse_params(id, params)?;
    let Some(current) = session.doc.texts.get(p.index) else {
        return Err(invalid(id, "text item does not exist on the active page"));
    };
    let mut updated = current.as_ref().clone();
    if !updated.toggle_checkbox_at_paragraph(p.paragraph_index) {
        return Err(invalid(
            id,
            "target paragraph does not contain a checkbox item",
        ));
    }
    updated = updated.sanitized();

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    if let Some(slot) = next.texts.get_mut(p.index) {
        *slot = Arc::new(updated.clone());
    }
    next.sync_active_to_pages();
    session.history.record("Toggle Checkbox", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();
    Ok(json!({ "textIndex": p.index, "text": updated.text }))
}

struct ValidatedSelectionIds {
    stroke_ids: Vec<StrokeId>,
    shape_indices: Vec<usize>,
    text_indices: Vec<usize>,
    image_indices: Vec<usize>,
}

fn validate_targets_list(
    doc: &Document,
    id: &str,
    targets: &[TransformTarget],
) -> Result<ValidatedSelectionIds, EngineError> {
    if targets.is_empty() || targets.len() > 8192 {
        return Err(invalid(
            id,
            "selection must contain between 1 and 8192 objects",
        ));
    }
    let stroke_set: HashSet<StrokeId> = doc.strokes.iter().map(|s| s.id).collect();
    let mut seen = HashSet::with_capacity(targets.len());
    let mut out = ValidatedSelectionIds {
        stroke_ids: Vec::new(),
        shape_indices: Vec::new(),
        text_indices: Vec::new(),
        image_indices: Vec::new(),
    };
    for &t in targets {
        if !seen.insert(t) {
            return Err(invalid(id, "selection contains a duplicate object"));
        }
        match t {
            TransformTarget::Stroke { id: sid } if stroke_set.contains(&sid) => {
                out.stroke_ids.push(sid);
            }
            TransformTarget::Shape { index } if index < doc.shapes.len() => {
                out.shape_indices.push(index);
            }
            TransformTarget::Text { index } if index < doc.texts.len() => {
                out.text_indices.push(index);
            }
            TransformTarget::Image { index } if index < doc.images.len() => {
                out.image_indices.push(index);
            }
            _ => {
                return Err(invalid(
                    id,
                    "a selected object no longer exists on the active page",
                ));
            }
        }
    }
    Ok(out)
}

fn expand_bbox_with_pt(bounds: &mut (f64, f64, f64, f64), has_pt: &mut bool, x: f64, y: f64) {
    if !x.is_finite() || !y.is_finite() {
        return;
    }
    if !*has_pt {
        *bounds = (x, y, x, y);
        *has_pt = true;
    } else {
        bounds.0 = bounds.0.min(x);
        bounds.1 = bounds.1.min(y);
        bounds.2 = bounds.2.max(x);
        bounds.3 = bounds.3.max(y);
    }
}

fn build_clipboard_from_selection(
    doc: &Document,
    sel: &ValidatedSelectionIds,
) -> PageSelectionClipboard {
    let mut bounds = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    let mut has_pt = false;
    let stroke_map: HashMap<StrokeId, &InkStroke> =
        doc.strokes.iter().map(|s| (s.id, s.as_ref())).collect();

    let mut raw_strokes = Vec::with_capacity(sel.stroke_ids.len());
    for &sid in &sel.stroke_ids {
        if let Some(&st) = stroke_map.get(&sid) {
            let clean = st.sanitized();
            for p in clean.smoothed_points.iter() {
                expand_bbox_with_pt(&mut bounds, &mut has_pt, p.x, p.y);
            }
            raw_strokes.push(clean);
        }
    }

    let mut raw_shapes = Vec::with_capacity(sel.shape_indices.len());
    for &idx in &sel.shape_indices {
        if let Some(sh) = doc.shapes.get(idx) {
            let clean = sh.sanitized();
            for (seg_pts, _) in clean.path_segments() {
                for p in seg_pts {
                    expand_bbox_with_pt(&mut bounds, &mut has_pt, p.x, p.y);
                }
            }
            raw_shapes.push(clean);
        }
    }

    let mut raw_texts = Vec::with_capacity(sel.text_indices.len());
    for &idx in &sel.text_indices {
        if let Some(tx) = doc.texts.get(idx) {
            let clean = tx.sanitized();
            let est_w = f64::from(clean.effective_max_width(doc.width)).min(360.0);
            let lines = clean.text.split('\n').count().max(1) as f64;
            let est_h = lines * f64::from(clean.size * clean.line_spacing);
            expand_bbox_with_pt(&mut bounds, &mut has_pt, clean.x, clean.y);
            expand_bbox_with_pt(&mut bounds, &mut has_pt, clean.x + est_w, clean.y + est_h);
            raw_texts.push(clean);
        }
    }

    let mut raw_images = Vec::with_capacity(sel.image_indices.len());
    for &idx in &sel.image_indices {
        if let Some(im) = doc.images.get(idx) {
            let clean = im.sanitized();
            for corner in clean.corners() {
                expand_bbox_with_pt(&mut bounds, &mut has_pt, corner.x, corner.y);
            }
            raw_images.push(clean);
        }
    }

    let (ox, oy, max_x, max_y) = if has_pt {
        bounds
    } else {
        (0.0, 0.0, 120.0, 120.0)
    };
    let shift_stroke = |st: &InkStroke| -> InkStroke {
        let shift_pts = |pts: &[StrokeSample]| -> Vec<StrokeSample> {
            pts.iter()
                .map(|s| StrokeSample {
                    x: s.x - ox,
                    y: s.y - oy,
                    ..*s
                })
                .collect()
        };
        InkStroke::new(
            st.id,
            st.pen.clone(),
            shift_pts(st.raw_points.as_slice()),
            shift_pts(st.smoothed_points.as_slice()),
        )
    };
    let strokes = raw_strokes.iter().map(shift_stroke).collect();
    let shapes = raw_shapes
        .into_iter()
        .map(|mut s| {
            s.x1 -= ox;
            s.y1 -= oy;
            s.x2 -= ox;
            s.y2 -= oy;
            for pt in &mut s.points {
                pt.x -= ox;
                pt.y -= oy;
            }
            s.sanitized()
        })
        .collect();
    let texts = raw_texts
        .into_iter()
        .map(|mut t| {
            t.x -= ox;
            t.y -= oy;
            t.sanitized()
        })
        .collect();
    let images = raw_images
        .into_iter()
        .map(|mut im| {
            im.x -= ox;
            im.y -= oy;
            im.sanitized()
        })
        .collect();

    PageSelectionClipboard {
        origin_x: ox,
        origin_y: oy,
        width: (max_x - ox).max(8.0),
        height: (max_y - oy).max(8.0),
        strokes,
        shapes,
        texts,
        images,
    }
}

struct StampedResult {
    stroke_ids: Vec<StrokeId>,
    shape_indices: Vec<usize>,
    text_indices: Vec<usize>,
    image_indices: Vec<usize>,
}

fn stamp_clipboard_onto_doc(
    next: &mut Document,
    bundle: &PageSelectionClipboard,
    dst_x: f64,
    dst_y: f64,
    scale: f64,
    id: &str,
) -> Result<StampedResult, EngineError> {
    if !dst_x.is_finite() || !dst_y.is_finite() || !scale.is_finite() || scale <= 0.0 {
        return Err(invalid(id, "stamp position and scale must be finite"));
    }
    let scale = scale.clamp(0.1, 10.0);
    if next.shapes.len().saturating_add(bundle.shapes.len()) > MAX_PAGE_SHAPES {
        return Err(invalid(id, "active page shape limit reached"));
    }
    if next.texts.len().saturating_add(bundle.texts.len()) > MAX_PAGE_TEXT_ITEMS {
        return Err(invalid(id, "active page text item limit reached"));
    }
    if next.images.len().saturating_add(bundle.images.len()) > MAX_PAGE_IMAGES {
        return Err(invalid(id, "active page image limit reached"));
    }
    let existing_chars: usize = next.texts.iter().map(|t| t.text.chars().count()).sum();
    let added_chars: usize = bundle.texts.iter().map(|t| t.text.chars().count()).sum();
    if existing_chars.saturating_add(added_chars) > MAX_PAGE_TEXT_CHARS {
        return Err(invalid(id, "active page text character limit reached"));
    }

    let mut out = StampedResult {
        stroke_ids: Vec::with_capacity(bundle.strokes.len()),
        shape_indices: Vec::with_capacity(bundle.shapes.len()),
        text_indices: Vec::with_capacity(bundle.texts.len()),
        image_indices: Vec::with_capacity(bundle.images.len()),
    };

    for st in &bundle.strokes {
        let new_id = next.alloc_stroke_id();
        let xform = |pts: &[StrokeSample]| -> Vec<StrokeSample> {
            pts.iter()
                .map(|s| StrokeSample {
                    x: dst_x + s.x * scale,
                    y: dst_y + s.y * scale,
                    ..*s
                })
                .collect()
        };
        let mut pen = st.pen.clone();
        pen.size = (f64::from(pen.size) * scale) as f32;
        let stroke = InkStroke::new(
            new_id,
            pen.sanitized(),
            xform(st.raw_points.as_slice()),
            xform(st.smoothed_points.as_slice()),
        );
        next.strokes.push(Arc::new(stroke));
        out.stroke_ids.push(new_id);
    }

    for sh in &bundle.shapes {
        let mut shape = sh.clone();
        shape.x1 = dst_x + shape.x1 * scale;
        shape.y1 = dst_y + shape.y1 * scale;
        shape.x2 = dst_x + shape.x2 * scale;
        shape.y2 = dst_y + shape.y2 * scale;
        for pt in &mut shape.points {
            pt.x = dst_x + pt.x * scale;
            pt.y = dst_y + pt.y * scale;
        }
        shape.width = (f64::from(shape.width) * scale) as f32;
        shape.curve *= scale;
        let idx = next.shapes.len();
        next.shapes.push(Arc::new(shape.sanitized()));
        out.shape_indices.push(idx);
    }

    for tx in &bundle.texts {
        let mut text = tx.clone();
        text.x = dst_x + text.x * scale;
        text.y = dst_y + text.y * scale;
        text.size = (f64::from(text.size) * scale).clamp(6.0, 120.0) as f32;
        text.max_width = text.max_width.map(|w| (f64::from(w) * scale) as f32);
        let idx = next.texts.len();
        next.texts.push(Arc::new(text.sanitized()));
        out.text_indices.push(idx);
    }

    for im in &bundle.images {
        let mut img = im.clone();
        img.x = dst_x + img.x * scale;
        img.y = dst_y + img.y * scale;
        img.width = (img.width * scale).max(16.0);
        img.height = (img.height * scale).max(16.0);
        let idx = next.images.len();
        next.images.push(Arc::new(img.sanitized()));
        out.image_indices.push(idx);
    }

    Ok(out)
}

#[derive(Deserialize)]
struct CopySelectionParams {
    targets: Vec<TransformTarget>,
    cut: Option<bool>,
}

fn cmd_selection_copy(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: CopySelectionParams = parse_params(id, params)?;
    let sel = validate_targets_list(&session.doc, id, &p.targets)?;
    let clip = build_clipboard_from_selection(&session.doc, &sel);
    let counts = (
        clip.strokes.len(),
        clip.shapes.len(),
        clip.texts.len(),
        clip.images.len(),
    );
    session.clipboard = Some(clip);

    let do_cut = p.cut.unwrap_or(false);
    if do_cut {
        let before = Arc::clone(&session.doc);
        let mut next = (*session.doc).clone();
        let stroke_set: HashSet<StrokeId> = sel.stroke_ids.into_iter().collect();
        if !stroke_set.is_empty() {
            next.strokes.retain(|s| !stroke_set.contains(&s.id));
        }
        let mut shape_indices = sel.shape_indices;
        shape_indices.sort_unstable_by(|a, b| b.cmp(a));
        for idx in shape_indices {
            next.shapes.remove(idx);
        }
        let mut text_indices = sel.text_indices;
        text_indices.sort_unstable_by(|a, b| b.cmp(a));
        for idx in text_indices {
            next.texts.remove(idx);
        }
        let mut image_indices = sel.image_indices;
        image_indices.sort_unstable_by(|a, b| b.cmp(a));
        for idx in image_indices {
            next.images.remove(idx);
        }
        next.sync_active_to_pages();
        session.history.record("Cut Selection", before);
        session.doc = Arc::new(next);
        session.mark_active_dirty();
        session.bump_revision();
    }

    Ok(json!({
        "cut": do_cut,
        "copied": {
            "strokes": counts.0,
            "shapes": counts.1,
            "texts": counts.2,
            "images": counts.3,
        }
    }))
}

#[derive(Deserialize)]
struct PasteSelectionParams {
    target_x: Option<f64>,
    target_y: Option<f64>,
    offset_x: Option<f64>,
    offset_y: Option<f64>,
}

fn cmd_selection_paste(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: PasteSelectionParams = parse_params(id, params)?;
    let Some(clip) = session.clipboard.clone() else {
        return Err(invalid(id, "workspace selection clipboard is empty"));
    };
    if clip.is_empty() {
        return Err(invalid(id, "workspace selection clipboard is empty"));
    }
    if p.target_x.is_some_and(|v| !v.is_finite())
        || p.target_y.is_some_and(|v| !v.is_finite())
        || p.offset_x.is_some_and(|v| !v.is_finite())
        || p.offset_y.is_some_and(|v| !v.is_finite())
    {
        return Err(invalid(id, "paste coordinates must be finite"));
    }

    let page_w = f64::from(session.doc.width);
    let page_h = f64::from(session.doc.height);
    let dst_x = p.target_x.unwrap_or_else(|| {
        (clip.origin_x + p.offset_x.unwrap_or(28.0)).clamp(0.0, (page_w - clip.width).max(0.0))
    });
    let dst_y = p.target_y.unwrap_or_else(|| {
        (clip.origin_y + p.offset_y.unwrap_or(28.0)).clamp(0.0, (page_h - clip.height).max(0.0))
    });

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let res = stamp_clipboard_onto_doc(&mut next, &clip, dst_x, dst_y, 1.0, id)?;
    next.sync_active_to_pages();
    session.history.record("Paste Selection", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();

    Ok(json!({
        "stroke_ids": res.stroke_ids.iter().map(|s| s.0).collect::<Vec<_>>(),
        "shape_indices": res.shape_indices,
        "text_indices": res.text_indices,
        "image_indices": res.image_indices,
    }))
}

#[derive(Deserialize)]
struct DuplicateSelectionParams {
    targets: Vec<TransformTarget>,
    offset_x: Option<f64>,
    offset_y: Option<f64>,
}

fn cmd_selection_duplicate(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DuplicateSelectionParams = parse_params(id, params)?;
    if p.offset_x.is_some_and(|v| !v.is_finite()) || p.offset_y.is_some_and(|v| !v.is_finite()) {
        return Err(invalid(id, "duplicate offset must be finite"));
    }
    let sel = validate_targets_list(&session.doc, id, &p.targets)?;
    let bundle = build_clipboard_from_selection(&session.doc, &sel);
    let page_w = f64::from(session.doc.width);
    let page_h = f64::from(session.doc.height);
    let dst_x =
        (bundle.origin_x + p.offset_x.unwrap_or(28.0)).clamp(0.0, (page_w - bundle.width).max(0.0));
    let dst_y = (bundle.origin_y + p.offset_y.unwrap_or(28.0))
        .clamp(0.0, (page_h - bundle.height).max(0.0));

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let res = stamp_clipboard_onto_doc(&mut next, &bundle, dst_x, dst_y, 1.0, id)?;
    next.sync_active_to_pages();
    session.history.record("Duplicate Selection", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();

    Ok(json!({
        "stroke_ids": res.stroke_ids.iter().map(|s| s.0).collect::<Vec<_>>(),
        "shape_indices": res.shape_indices,
        "text_indices": res.text_indices,
        "image_indices": res.image_indices,
    }))
}

#[derive(Deserialize)]
struct SaveElementParams {
    name: String,
    targets: Vec<TransformTarget>,
}

fn cmd_element_save(session: &mut Session, id: &str, params: &Value) -> Result<Value, EngineError> {
    let p: SaveElementParams = parse_params(id, params)?;
    let sel = validate_targets_list(&session.doc, id, &p.targets)?;
    if sel.stroke_ids.len() > MAX_ELEMENT_STROKES
        || sel.shape_indices.len() > MAX_ELEMENT_SHAPES
        || sel.text_indices.len() > MAX_ELEMENT_TEXTS
        || sel.image_indices.len() > MAX_ELEMENT_IMAGES
    {
        return Err(invalid(id, "selection exceeds saved element object limit"));
    }
    let bundle = build_clipboard_from_selection(&session.doc, &sel);
    if bundle.strokes.is_empty()
        && bundle.shapes.is_empty()
        && bundle.texts.is_empty()
        && bundle.images.is_empty()
    {
        return Err(invalid(
            id,
            "select at least one stroke, shape, text item, or image to save an element",
        ));
    }
    if session.prefs.saved_elements.len() >= MAX_SAVED_ELEMENTS {
        return Err(invalid(id, "saved elements limit reached"));
    }

    let existing_ids: Vec<&str> = session
        .prefs
        .saved_elements
        .iter()
        .map(|el| el.id.as_str())
        .collect();
    let new_id = allocate_unique_id(&existing_ids, None, "el");
    let clean_name: String = p
        .name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ELEMENT_NAME_LEN)
        .collect();
    let candidate = SavedElement {
        id: new_id.clone(),
        name: if clean_name.trim().is_empty() {
            "Sticker".to_string()
        } else {
            clean_name.trim().to_string()
        },
        built_in: false,
        width: bundle.width,
        height: bundle.height,
        strokes: bundle.strokes,
        shapes: bundle.shapes,
        texts: bundle.texts,
        images: bundle.images,
    };
    let Some(clean_el) = candidate.sanitized() else {
        return Err(invalid(id, "saved element cannot be empty"));
    };
    session.prefs.saved_elements.push(clean_el.clone());
    let _ = session.save_prefs();
    session.bump_revision();

    Ok(json!({
        "element": clean_el,
        "count": session.prefs.saved_elements.len(),
    }))
}

#[derive(Deserialize)]
struct PlaceElementParams {
    id: String,
    x: Option<f64>,
    y: Option<f64>,
    scale: Option<f64>,
}

fn cmd_element_place(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: PlaceElementParams = parse_params(id, params)?;
    if p.x.is_some_and(|v| !v.is_finite())
        || p.y.is_some_and(|v| !v.is_finite())
        || p.scale.is_some_and(|v| !v.is_finite() || v <= 0.0)
    {
        return Err(invalid(
            id,
            "placement coordinates and scale must be finite",
        ));
    }
    let req_id = sanitize_preset_id(&p.id);
    let found = SavedElement::built_ins()
        .into_iter()
        .chain(session.prefs.saved_elements.iter().cloned())
        .find(|el| el.id == req_id)
        .ok_or_else(|| invalid(id, format!("unknown saved element `{}`", p.id)))?;

    let scale = p.scale.unwrap_or(1.0).clamp(0.2, 4.0);
    let page_w = f64::from(session.doc.width);
    let page_h = f64::from(session.doc.height);
    let center_x = p.x.unwrap_or(page_w * 0.5);
    let center_y = p.y.unwrap_or(page_h * 0.5);
    let dst_x =
        (center_x - found.width * 0.5 * scale).clamp(0.0, (page_w - found.width * scale).max(0.0));
    let dst_y = (center_y - found.height * 0.5 * scale)
        .clamp(0.0, (page_h - found.height * scale).max(0.0));

    let bundle = PageSelectionClipboard {
        origin_x: dst_x,
        origin_y: dst_y,
        width: found.width * scale,
        height: found.height * scale,
        strokes: found.strokes,
        shapes: found.shapes,
        texts: found.texts,
        images: found.images,
    };

    let before = Arc::clone(&session.doc);
    let mut next = (*session.doc).clone();
    let res = stamp_clipboard_onto_doc(&mut next, &bundle, dst_x, dst_y, scale, id)?;
    next.sync_active_to_pages();
    session.history.record("Place Element", before);
    session.doc = Arc::new(next);
    session.mark_active_dirty();
    session.bump_revision();

    Ok(json!({
        "element_id": found.id,
        "stroke_ids": res.stroke_ids.iter().map(|s| s.0).collect::<Vec<_>>(),
        "shape_indices": res.shape_indices,
        "text_indices": res.text_indices,
        "image_indices": res.image_indices,
    }))
}

#[derive(Deserialize)]
struct DeleteElementParams {
    id: String,
}

fn cmd_element_delete(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: DeleteElementParams = parse_params(id, params)?;
    let req_id = sanitize_preset_id(&p.id);
    if SavedElement::built_ins().iter().any(|b| b.id == req_id) {
        return Err(invalid(id, "built-in starter elements cannot be deleted"));
    }
    let Some(pos) = session
        .prefs
        .saved_elements
        .iter()
        .position(|e| e.id == req_id)
    else {
        return Err(invalid(id, format!("unknown saved element `{}`", p.id)));
    };
    session.prefs.saved_elements.remove(pos);
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "deleted_id": req_id,
        "count": session.prefs.saved_elements.len(),
    }))
}

#[derive(Deserialize)]
struct SaveStyleFavoriteParams {
    kind: Option<PenKind>,
    color: Option<Color>,
    size: Option<f32>,
}

fn cmd_style_favorite_save(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: SaveStyleFavoriteParams = parse_params(id, params)?;
    if p.size.is_some_and(|s| !s.is_finite() || s <= 0.0) {
        return Err(invalid(id, "favorite size must be finite and positive"));
    }
    let kind = p.kind.unwrap_or_else(|| session.active_pen_kind());
    let base_pen = session.pen(kind);
    let color = p
        .color
        .unwrap_or(base_pen.color)
        .sanitized()
        .with_alpha(1.0);
    let size = p.size.unwrap_or(base_pen.size).clamp(0.5, 120.0);

    if let Some(existing) = session.prefs.style_favorites.iter().find(|f| {
        f.kind == kind && f.color.to_hex() == color.to_hex() && (f.size - size).abs() < 0.05
    }) {
        return Ok(json!({
            "favorite": existing,
            "count": session.prefs.style_favorites.len(),
        }));
    }
    if session.prefs.style_favorites.len() >= MAX_STYLE_FAVORITES {
        return Err(invalid(id, "toolbar style favorites limit reached (max 8)"));
    }

    let existing_ids: Vec<&str> = session
        .prefs
        .style_favorites
        .iter()
        .map(|f| f.id.as_str())
        .collect();
    let fav_id = allocate_unique_id(&existing_ids, None, "fav");
    let fav = StyleFavorite {
        id: fav_id,
        kind,
        color,
        size,
    }
    .sanitized();
    session.prefs.style_favorites.push(fav.clone());
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "favorite": fav,
        "count": session.prefs.style_favorites.len(),
    }))
}

#[derive(Deserialize)]
struct StyleFavoriteIdParams {
    id: String,
}

fn cmd_style_favorite_apply(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: StyleFavoriteIdParams = parse_params(id, params)?;
    let req_id = sanitize_preset_id(&p.id);
    let Some(fav) = session
        .prefs
        .style_favorites
        .iter()
        .find(|f| f.id == req_id)
        .cloned()
    else {
        return Err(invalid(id, format!("unknown style favorite `{}`", p.id)));
    };
    {
        let pen = session.pen_mut(fav.kind);
        pen.color = fav.color;
        pen.size = fav.size;
        *pen = pen.sanitized();
    }
    remember_pen_size(session, fav.kind, fav.size);
    session.set_active_tool(ToolKind::Pen(fav.kind));
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({ "favorite": fav }))
}

fn cmd_style_favorite_delete(
    session: &mut Session,
    id: &str,
    params: &Value,
) -> Result<Value, EngineError> {
    let p: StyleFavoriteIdParams = parse_params(id, params)?;
    let req_id = sanitize_preset_id(&p.id);
    let Some(pos) = session
        .prefs
        .style_favorites
        .iter()
        .position(|f| f.id == req_id)
    else {
        return Err(invalid(id, format!("unknown style favorite `{}`", p.id)));
    };
    session.prefs.style_favorites.remove(pos);
    let _ = session.save_prefs();
    session.bump_revision();
    Ok(json!({
        "deleted_id": req_id,
        "count": session.prefs.style_favorites.len(),
    }))
}
