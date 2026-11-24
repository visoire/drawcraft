//! L5 headless engine session and command registry.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod commands;
pub mod library;
pub mod study;
pub mod sync_state;

pub use commands::{
    PackageImportSummary, PdfImportMode, PreparedPdfImport, prepare_pdf_import_from_bytes,
    prepare_pdf_import_from_path,
};
pub use drawcraft_compose::{
    PdfDocumentSummary, PdfError, PdfPageRenderer, ROBOTO_BOLD_ITALIC_TTF, ROBOTO_BOLD_TTF,
    ROBOTO_ITALIC_TTF, ROBOTO_REGULAR_TTF, RenderedPdfPage,
};
pub use drawcraft_format::looks_like_package_bytes;
/// Tiered storage model (hot LRU, byte caps, revision-bound flush) for UI shells.
pub use drawcraft_storage as storage;
pub use library::{TrashedNotebook, default_library_dir};
pub use sync_state::{AuthWorkspaceMode, LocalMigrationSummary, LocalNotebookEntry};

use std::sync::Arc;

use drawcraft_doc::{
    Document, EraserMode, GeometricShape, HighlighterMode, InkStroke, MAX_PAGE_SHAPES,
    PageSelectionClipboard, PenKind, PenSettings, StrokeId, StrokeSample, UserStudioConfig,
};
use drawcraft_geom::Point;
use drawcraft_ops::History;
use drawcraft_paint::eraser::erase_along_path_filtered;
use drawcraft_paint::spline::{densify_linear, predict_live_tip, resample_centripetal_catmull_rom};
use drawcraft_paint::stabilizer::StrokeStabilizer;
use drawcraft_paint::{
    finalize_smoothed_samples, recognize_drawn_shape, simplify_committed_spine,
    smooth_stroke_samples,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("invalid parameters for `{command}`: {reason}")]
    InvalidParams { command: String, reason: String },
}

/// Active selection, drawing, or erasing tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolKind {
    Select,
    Pen(PenKind),
    Eraser,
    LaserPointer,
}

impl Default for ToolKind {
    fn default() -> Self {
        Self::Pen(PenKind::FountainPen)
    }
}

/// Result committed when finishing an interactive canvas draw stroke.
#[derive(Clone, Debug, PartialEq)]
pub enum FinishedLiveDraw {
    Stroke(StrokeId),
    Shape { index: usize, shape: GeometricShape },
}

/// In-progress ink stroke before commit.
#[derive(Clone, Debug)]
pub struct LiveStroke {
    pub pen: PenSettings,
    pub zoom: f32,
    pub straight_line: bool,
    pub straight_line_locked: bool,
    pub recognized_shape: Option<GeometricShape>,
    pub stabilizer: StrokeStabilizer,
    pub raw_points: Vec<StrokeSample>,
    pub stabilized_points: Vec<StrokeSample>,
    pub smoothed_preview: Vec<StrokeSample>,
    pub preview_dirty: bool,
}

impl LiveStroke {
    pub fn preview_samples(&self) -> Vec<StrokeSample> {
        let mut out = self.smoothed_preview.clone();
        if !self.straight_line
            && let Some(tip) = predict_live_tip(&out, self.pen.smoothing.prediction_ms)
        {
            out.push(tip);
        }
        out
    }

    pub fn string_tether(&self) -> Option<(Point, Point)> {
        self.stabilizer.string_tether()
    }
}

/// In-progress eraser drag.
#[derive(Clone, Debug)]
pub struct LiveEraser {
    pub mode: EraserMode,
    pub highlighter_only: bool,
    pub radius: f64,
    pub path: Vec<(Point, f64)>,
    pub snapshot_before: Arc<Document>,
    pub modified: bool,
}

/// Maximum number of open notebooks kept in one workspace session.
pub const MAX_NOTEBOOKS: usize = 256;

/// Complete headless DrawCraft editing session.
#[derive(Clone, Debug)]
pub struct Session {
    pub doc: Arc<Document>,
    pub notebooks: Vec<Arc<Document>>,
    pub trashed_notebooks: Vec<TrashedNotebook>,
    pub active_notebook: usize,
    pub last_export_status: Option<String>,
    pub revision: u64,
    pub history: History,
    pub active_tool: ToolKind,
    pub last_pen_kind: PenKind,
    pub pens: [PenSettings; 5],
    pub eraser_mode: EraserMode,
    pub eraser_size: f32,
    pub eraser_highlighter_only: bool,
    pub highlighter_mode: HighlighterMode,
    pub straight_line_hold: bool,
    pub zoom: f32,
    pub live_stroke: Option<LiveStroke>,
    pub live_eraser: Option<LiveEraser>,
    pub clipboard: Option<PageSelectionClipboard>,
    pub prefs: UserStudioConfig,
    pub(crate) library: library::LibraryState,
    pub(crate) sync: sync_state::SessionSyncState,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Document::default())
    }
}

impl Session {
    pub fn new(mut doc: Document) -> Self {
        doc.sync_active_to_pages();
        let doc = Arc::new(doc);
        let mut session = Self {
            doc: Arc::clone(&doc),
            notebooks: vec![doc],
            trashed_notebooks: Vec::new(),
            active_notebook: 0,
            last_export_status: None,
            revision: 1,
            history: History::default(),
            active_tool: ToolKind::Pen(PenKind::FountainPen),
            last_pen_kind: PenKind::FountainPen,
            pens: [
                PenSettings::fountain_pen(),
                PenSettings::ball_pen(),
                PenSettings::brush_pen(),
                PenSettings::highlighter(),
                PenSettings::pencil(),
            ],
            eraser_mode: EraserMode::StrokeEraser,
            eraser_size: 22.0,
            eraser_highlighter_only: false,
            highlighter_mode: HighlighterMode::Freehand,
            straight_line_hold: false,
            zoom: 1.0,
            live_stroke: None,
            live_eraser: None,
            clipboard: None,
            prefs: UserStudioConfig::default(),
            library: library::LibraryState {
                slot_files: vec![None],
                slot_dirty: vec![false],
                ..Default::default()
            },
            sync: sync_state::SessionSyncState::default(),
        };
        session.apply_pen_size_preferences();
        session
    }

    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value, EngineError> {
        commands::execute(self, id, params)
    }

    /// Sets the active tool and updates [`Self::last_pen_kind`] whenever a pen tool is chosen.
    pub fn set_active_tool(&mut self, tool: ToolKind) {
        self.active_tool = tool;
        if let ToolKind::Pen(kind) = tool {
            self.last_pen_kind = kind;
        }
    }

    pub fn active_pen_kind(&self) -> PenKind {
        match self.active_tool {
            ToolKind::Pen(k) => k,
            ToolKind::Select | ToolKind::Eraser | ToolKind::LaserPointer => self.last_pen_kind,
        }
    }

    pub fn pen(&self, kind: PenKind) -> &PenSettings {
        &self.pens[kind.index()]
    }

    pub fn pen_mut(&mut self, kind: PenKind) -> &mut PenSettings {
        &mut self.pens[kind.index()]
    }

    pub(crate) fn apply_pen_size_preferences(&mut self) {
        let sizes = self.prefs.pen_sizes;
        for kind in PenKind::ALL {
            if let (Some(pen), Some(size)) =
                (self.pens.get_mut(kind.index()), sizes.get(kind.index()))
            {
                pen.size = *size;
            }
        }
    }

    pub fn active_pen(&self) -> &PenSettings {
        self.pen(self.active_pen_kind())
    }

    pub fn active_pen_mut(&mut self) -> &mut PenSettings {
        let kind = self.active_pen_kind();
        self.pen_mut(kind)
    }

    pub fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    pub fn is_straight_line_active(&self, kind: PenKind) -> bool {
        self.straight_line_hold
            || (kind == PenKind::Highlighter && self.highlighter_mode == HighlighterMode::Straight)
    }

    pub fn begin_live_stroke(&mut self, sample: StrokeSample) {
        self.cancel_live_stroke();
        self.cancel_live_eraser();
        let pen = self.active_pen().sanitized();
        let straight_line = self.is_straight_line_active(pen.kind);
        let s = sample.sanitized();
        let mut stab = StrokeStabilizer::new(&pen.smoothing, self.zoom);
        let mut stabilized = Vec::with_capacity(64);
        stab.push(s, &mut stabilized);
        if stabilized.is_empty() {
            stabilized.push(s);
        }
        let preview = stabilized.clone();
        self.live_stroke = Some(LiveStroke {
            pen,
            zoom: self.zoom,
            straight_line,
            straight_line_locked: false,
            recognized_shape: None,
            stabilizer: stab,
            raw_points: vec![s],
            stabilized_points: stabilized,
            smoothed_preview: preview,
            preview_dirty: false,
        });
    }

    pub fn update_live_stroke(&mut self, sample: StrokeSample) {
        self.update_live_stroke_inner(sample, true);
    }

    /// Pushes a raw pointer sample into the live stroke without immediately re-evaluating
    /// the full preview spline, allowing multiple input samples per frame to coalesce.
    pub fn update_live_stroke_deferred(&mut self, sample: StrokeSample) {
        self.update_live_stroke_inner(sample, false);
    }

    /// Rebuilds the live stroke preview spline if deferred samples arrived since the last refresh.
    pub fn refresh_live_stroke_preview(&mut self) {
        let Some(live) = self.live_stroke.as_mut() else {
            return;
        };
        if !live.preview_dirty {
            return;
        }
        if live.straight_line
            && let (Some(&first), Some(&last)) = (live.raw_points.first(), live.raw_points.last())
        {
            live.smoothed_preview = densify_linear(&[first, last], 3.0);
        } else {
            refresh_live_preview(live);
        }
        live.preview_dirty = false;
    }

    fn update_live_stroke_inner(&mut self, sample: StrokeSample, refresh_now: bool) {
        let straight = self.live_stroke.as_ref().is_some_and(|live| {
            live.straight_line_locked || self.is_straight_line_active(live.pen.kind)
        });
        let Some(live) = self.live_stroke.as_mut() else {
            return;
        };
        if live.recognized_shape.is_some() {
            // Once a held stroke snaps into a recognized geometric shape, allow small tremor without dropping the shape,
            // or release to commit cleanly.
            return;
        }
        live.straight_line = straight;
        let s = sample.sanitized();
        live.raw_points.push(s);
        if straight && let Some(&first) = live.raw_points.first() {
            if refresh_now {
                live.smoothed_preview = densify_linear(&[first, s], 3.0);
                live.preview_dirty = false;
            } else {
                live.preview_dirty = true;
            }
            return;
        }
        live.stabilizer.push(s, &mut live.stabilized_points);
        if refresh_now {
            refresh_live_preview(live);
            live.preview_dirty = false;
        } else {
            live.preview_dirty = true;
        }
    }

    /// Evaluates the current live stroke for freehand geometric shape recognition ("Draw & Hold").
    /// Flushes a clone of the active stabilizer so held endpoints catch up even under buffered smoothing.
    /// Returns `true` if a clean closed shape or arrow was recognized and latched.
    pub fn lock_live_stroke_recognized_shape(&mut self) -> bool {
        let Some(live) = self.live_stroke.as_mut() else {
            return false;
        };
        if live.straight_line || live.straight_line_locked || live.recognized_shape.is_some() {
            return false;
        }
        let mut pts = live.stabilized_points.clone();
        let mut stab = live.stabilizer.clone();
        stab.finish(&mut pts);
        let caught_up = if pts.len() >= 6 {
            finalize_smoothed_samples(&pts, &live.pen.smoothing)
        } else {
            live.raw_points.clone()
        };
        let samples = if caught_up.len() >= 6 {
            &caught_up
        } else {
            &live.raw_points
        };
        if let Some(shape) = recognize_drawn_shape(samples, live.pen.color, live.pen.size, false) {
            live.recognized_shape = Some(shape);
            true
        } else {
            false
        }
    }

    /// Converts the current live stroke to a line and keeps it straight until release.
    pub fn lock_live_stroke_straight(&mut self) -> bool {
        let Some(live) = self.live_stroke.as_mut() else {
            return false;
        };
        if live.recognized_shape.is_some() || live.raw_points.len() < 2 {
            return false;
        }
        let (Some(&first), Some(&last)) = (live.raw_points.first(), live.raw_points.last()) else {
            return false;
        };
        if first.point().distance(last.point()) < 1e-3 {
            return false;
        }
        live.straight_line = true;
        live.straight_line_locked = true;
        live.smoothed_preview = densify_linear(&[first, last], 3.0);
        live.preview_dirty = false;
        true
    }

    pub fn tick_live_catch_up(&mut self, now_ms: f64) -> bool {
        if self.live_stroke.as_ref().is_some_and(|live| {
            live.recognized_shape.is_some()
                || live.straight_line_locked
                || self.is_straight_line_active(live.pen.kind)
        }) {
            return false;
        }
        let Some(live) = self.live_stroke.as_mut() else {
            return false;
        };
        if !live.stabilizer.is_lagging() {
            return false;
        }
        let Some(mut last) = live.raw_points.last().copied() else {
            return false;
        };
        if now_ms.is_finite() && now_ms > last.time_ms {
            last.time_ms = now_ms;
        }
        let before = live.stabilized_points.len();
        live.stabilizer.push(last, &mut live.stabilized_points);
        if live.stabilized_points.len() != before {
            refresh_live_preview(live);
            live.preview_dirty = false;
            true
        } else {
            false
        }
    }

    pub fn finish_live_stroke(&mut self) -> Option<StrokeId> {
        match self.finish_live_stroke_with_shape()? {
            FinishedLiveDraw::Stroke(id) => Some(id),
            FinishedLiveDraw::Shape { .. } => None,
        }
    }

    /// Completes the live draw stroke, committing either a snapped [`GeometricShape`] or an [`InkStroke`].
    pub fn finish_live_stroke_with_shape(&mut self) -> Option<FinishedLiveDraw> {
        let mut live = self.live_stroke.take()?;
        if let Some(shape) = live.recognized_shape {
            let clean = shape.sanitized();
            if self.doc.shapes.len() < MAX_PAGE_SHAPES {
                let before = Arc::clone(&self.doc);
                let mut next = (*self.doc).clone();
                let index = next.shapes.len();
                next.shapes.push(Arc::new(clean.clone()));
                next.sync_active_to_pages();
                self.history.record("Draw Shape", before);
                self.doc = Arc::new(next);
                self.mark_active_dirty();
                self.bump_revision();
                return Some(FinishedLiveDraw::Shape {
                    index,
                    shape: clean,
                });
            }
        }
        if live.raw_points.is_empty() {
            return None;
        }
        let straight = live.straight_line_locked
            || live.straight_line
            || self.is_straight_line_active(live.pen.kind);
        let densified = if straight
            && live.raw_points.len() >= 2
            && let (Some(&first), Some(&last)) = (live.raw_points.first(), live.raw_points.last())
        {
            resample_centripetal_catmull_rom(&[first, last], 0.0, 4.0)
        } else {
            live.stabilizer.finish(&mut live.stabilized_points);
            if live.stabilized_points.is_empty() {
                live.stabilized_points.push(live.raw_points[0]);
            }
            finalize_smoothed_samples(&live.stabilized_points, &live.pen.smoothing)
        };
        let smoothed = simplify_committed_spine(&densified, &live.pen);

        self.commit_stroke(live.pen, live.raw_points, smoothed)
            .map(FinishedLiveDraw::Stroke)
    }

    pub fn cancel_live_stroke(&mut self) {
        self.live_stroke = None;
    }

    pub fn commit_stroke(
        &mut self,
        pen: PenSettings,
        raw_points: Vec<StrokeSample>,
        smoothed_points: Vec<StrokeSample>,
    ) -> Option<StrokeId> {
        if raw_points.is_empty() || smoothed_points.is_empty() {
            return None;
        }
        let before = Arc::clone(&self.doc);
        let mut next = (*self.doc).clone();
        let id = next.alloc_stroke_id();
        next.strokes.push(Arc::new(InkStroke::new(
            id,
            pen,
            raw_points,
            smoothed_points,
        )));
        let now = drawcraft_sync_protocol::now_unix_ms();
        if next.created_at_ms == 0 {
            next.created_at_ms = now;
        }
        next.updated_at_ms = now.max(next.updated_at_ms);
        next.sync_active_to_pages();
        self.history.record("Draw Stroke", before);
        self.doc = Arc::new(next);
        self.sync_active_notebook_slot();
        self.library
            .mark_dirty(self.active_notebook, self.notebooks.len());
        self.sync_mark_slot_dirty(self.active_notebook);
        self.bump_revision();
        Some(id)
    }

    pub fn commit_raw_stroke(
        &mut self,
        pen: PenSettings,
        raw_points: Vec<StrokeSample>,
    ) -> Option<StrokeId> {
        let densified = if self.is_straight_line_active(pen.kind)
            && raw_points.len() >= 2
            && let (Some(&first), Some(&last)) = (raw_points.first(), raw_points.last())
        {
            resample_centripetal_catmull_rom(&[first, last], 0.0, 4.0)
        } else {
            smooth_stroke_samples(&raw_points, &pen.smoothing, self.zoom)
        };
        let smoothed = simplify_committed_spine(&densified, &pen);
        self.commit_stroke(pen, raw_points, smoothed)
    }

    pub fn begin_live_eraser(&mut self, pt: Point) {
        self.cancel_live_stroke();
        self.cancel_live_eraser();
        if !pt.is_finite() {
            return;
        }
        let radius = f64::from((self.eraser_size * 0.5).clamp(1.0, 80.0));
        let mut live = LiveEraser {
            mode: self.eraser_mode,
            highlighter_only: self.eraser_highlighter_only,
            radius,
            path: vec![(pt, radius)],
            snapshot_before: Arc::clone(&self.doc),
            modified: false,
        };
        apply_live_eraser_step(self, &mut live, &[(pt, radius)]);
        self.live_eraser = Some(live);
    }

    pub fn update_live_eraser(&mut self, pt: Point) {
        if !pt.is_finite() {
            return;
        }
        let Some(mut live) = self.live_eraser.take() else {
            return;
        };
        let radius = live.radius;
        let prev = live.path.last().copied().unwrap_or((pt, radius));
        live.path.push((pt, radius));
        apply_live_eraser_step(self, &mut live, &[prev, (pt, radius)]);
        self.live_eraser = Some(live);
    }

    pub fn finish_live_eraser(&mut self) -> bool {
        let Some(live) = self.live_eraser.take() else {
            return false;
        };
        if live.modified {
            self.history.record("Erase", live.snapshot_before);
            let now = drawcraft_sync_protocol::now_unix_ms();
            let mut touched = (*self.doc).clone();
            if touched.created_at_ms == 0 {
                touched.created_at_ms = now;
            }
            touched.updated_at_ms = now.max(touched.updated_at_ms);
            self.doc = Arc::new(touched);
            self.sync_active_notebook_slot();
            self.library
                .mark_dirty(self.active_notebook, self.notebooks.len());
            self.sync_mark_slot_dirty(self.active_notebook);
            true
        } else {
            false
        }
    }

    pub fn cancel_live_eraser(&mut self) {
        if let Some(live) = self.live_eraser.take()
            && live.modified
        {
            self.doc = live.snapshot_before;
            self.sync_active_notebook_slot();
            self.bump_revision();
        }
    }

    /// Mirrors `self.doc` into the active slot when the library has an open notebook.
    pub fn sync_active_notebook_slot(&mut self) {
        if self.notebooks.is_empty() {
            self.active_notebook = 0;
            self.library.sync_len(0);
            self.sync_reconcile_slots();
            return;
        }
        if self.active_notebook >= self.notebooks.len() {
            self.active_notebook = self.notebooks.len().saturating_sub(1);
        }
        self.notebooks[self.active_notebook] = Arc::clone(&self.doc);
        self.library.sync_len(self.notebooks.len());
        self.sync_reconcile_slots();
    }

    /// Marks the active notebook slot as having unsaved changes for library persistence.
    pub fn mark_active_dirty(&mut self) {
        self.library
            .mark_dirty(self.active_notebook, self.notebooks.len());
        self.sync_mark_slot_dirty(self.active_notebook);
    }

    /// Updates `updated_at_ms` (and backfills `created_at_ms` if zero) on the active document.
    pub(crate) fn bump_active_doc_modified_timestamp(&mut self) {
        let now = drawcraft_sync_protocol::now_unix_ms();
        let next = Arc::make_mut(&mut self.doc);
        if next.created_at_ms == 0 {
            next.created_at_ms = now;
        }
        next.updated_at_ms = now.max(next.updated_at_ms);
    }

    /// Switches the active notebook index inside `self.notebooks`.
    pub fn switch_notebook(&mut self, index: usize) -> bool {
        self.sync_active_notebook_slot();
        if index >= self.notebooks.len() {
            return false;
        }
        if index == self.active_notebook {
            return true;
        }
        self.cancel_live_stroke();
        self.cancel_live_eraser();
        self.active_notebook = index;
        self.doc = Arc::clone(&self.notebooks[index]);
        self.history.clear();
        self.bump_revision();
        true
    }
}

fn apply_live_eraser_step(session: &mut Session, live: &mut LiveEraser, seg: &[(Point, f64)]) {
    let mut seg_bounds = drawcraft_geom::Rect::from_point_iter(seg.iter().map(|(p, _)| *p));
    let max_r = seg
        .iter()
        .map(|(_, r)| if r.is_finite() { r.max(0.0) } else { 0.0 })
        .fold(0.0_f64, f64::max);
    seg_bounds = seg_bounds.inflated(max_r + 2.0);
    let has_candidate = session.doc.strokes.iter().any(|s| {
        (!live.highlighter_only || s.pen.kind == PenKind::Highlighter)
            && s.bounds.intersects(seg_bounds)
    });
    if !has_candidate {
        return;
    }
    let mut next_id = session.doc.next_stroke_id;
    let (updated, changed) = erase_along_path_filtered(
        &session.doc.strokes,
        seg,
        live.mode,
        live.highlighter_only,
        &mut next_id,
    );
    if changed {
        let mut next = (*session.doc).clone();
        next.next_stroke_id = next_id;
        next.strokes = updated;
        next.sync_active_to_pages();
        session.doc = Arc::new(next);
        session.bump_revision();
        live.modified = true;
    }
}

fn refresh_live_preview(live: &mut LiveStroke) {
    live.smoothed_preview = finalize_smoothed_samples(&live.stabilized_points, &live.pen.smoothing);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod pdf_command_tests {
    use super::*;
    use serde_json::{Value, json};

    fn sample_two_page_pdf() -> Vec<u8> {
        let objects = [
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".as_slice(),
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n".as_slice(),
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>\nendobj\n".as_slice(),
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Resources << >> >>\nendobj\n".as_slice(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        for obj in objects {
            offsets.push(pdf.len());
            pdf.extend_from_slice(obj);
        }
        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for off in &offsets[1..] {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        pdf
    }

    #[test]
    fn pdf_import_creates_annotated_notebook_and_preserves_backdrop_on_clear() {
        let mut s = Session::default();
        let raw_pdf = sample_two_page_pdf();
        let res = s
            .import_pdf_bytes(
                &raw_pdf,
                Some("Lecture Notes.pdf"),
                PdfImportMode::NewNotebook,
                false,
            )
            .expect("import pdf");
        assert_eq!(res["mode"], "new_notebook");
        assert_eq!(res["pageCount"], 2);
        assert_eq!(s.doc.title, "Lecture Notes");
        assert_eq!(s.doc.width, 600);
        assert_eq!(s.doc.height, 800);
        assert_eq!(s.doc.pdf_documents.len(), 1);
        assert!(s.doc.pdf_backdrop.is_some());

        // Switch to page 2 (landscape PDF page)
        s.execute("page.select", &json!({ "index": 1 })).unwrap();
        assert_eq!(s.doc.width, 800);
        assert_eq!(s.doc.height, 400);

        // Draw ink and clear page: PDF backdrop must remain intact!
        s.commit_raw_stroke(
            PenSettings::ball_pen(),
            vec![
                StrokeSample::from_xy_pressure(20.0, 20.0, 0.7),
                StrokeSample::from_xy_pressure(80.0, 40.0, 0.7),
            ],
        );
        assert_eq!(s.doc.strokes.len(), 1);
        s.execute("page.clear", &Value::Null).unwrap();
        assert!(s.doc.strokes.is_empty());
        assert!(s.doc.pdf_backdrop.is_some());
    }

    #[test]
    fn pdf_import_insert_pages_deduplicates_supports_undo_and_prunes_on_remove() {
        let mut s = Session::default();
        let raw_pdf = sample_two_page_pdf();

        // First insert adds pdf_documents[0] and 2 pages after page 0
        let first = s
            .import_pdf_bytes(
                &raw_pdf,
                Some("Slides.pdf"),
                PdfImportMode::InsertPages,
                false,
            )
            .expect("insert pdf pages");
        assert_eq!(first["insertedPages"], 2);
        assert_eq!(first["startPageIndex"], 1);
        assert_eq!(s.doc.page_count(), 3);
        assert_eq!(s.doc.pdf_documents.len(), 1);

        // Undo removes the inserted pages and restores non-PDF document cleanly
        let undone = s.execute("edit.undo", &Value::Null).unwrap();
        assert_eq!(undone["undone"], true);
        assert_eq!(s.doc.page_count(), 1);
        assert!(s.doc.pdf_documents.is_empty());

        // Redo restores them
        let redone = s.execute("edit.redo", &Value::Null).unwrap();
        assert_eq!(redone["redone"], true);
        assert_eq!(s.doc.page_count(), 3);
        assert_eq!(s.doc.pdf_documents.len(), 1);

        // Inserting the exact same PDF again reuses pdf_index 0 without duplicating bytes
        let second = s
            .import_pdf_bytes(
                &raw_pdf,
                Some("Slides.pdf"),
                PdfImportMode::InsertPages,
                false,
            )
            .expect("reinsert same pdf");
        assert_eq!(second["pdfIndex"], 0);
        assert_eq!(s.doc.pdf_documents.len(), 1);
        assert_eq!(s.doc.page_count(), 5);

        // Remove all PDF pages -> unused PDF source is pruned automatically
        for idx in (1..=4).rev() {
            s.execute("page.remove", &json!({ "index": idx })).unwrap();
        }
        assert_eq!(s.doc.page_count(), 1);
        assert!(s.doc.pdf_documents.is_empty());
        assert_eq!(s.doc.pdf_backdrop, None);
    }

    #[test]
    fn pdf_import_command_rejects_corrupt_and_missing_inputs_without_panicking() {
        let mut s = Session::default();
        assert!(s.execute("pdf.import", &Value::Null).is_err());
        assert!(
            s.execute(
                "pdf.import",
                &json!({ "base64_bytes": drawcraft_doc::base64_encode_bytes(b"not a pdf") })
            )
            .is_err()
        );
    }

    fn sample_large_pdf(extra_stream_bytes: usize) -> Vec<u8> {
        let obj1 = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
        let obj2 = b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
        let obj3 = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>\nendobj\n";
        let obj4_head = format!("4 0 obj\n<< /Length {extra_stream_bytes} >>\nstream\n");
        let obj4_tail = b"\nendstream\nendobj\n";

        let mut pdf = Vec::with_capacity(256 + extra_stream_bytes);
        pdf.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = vec![0usize];
        for obj in [obj1.as_slice(), obj2.as_slice(), obj3.as_slice()] {
            offsets.push(pdf.len());
            pdf.extend_from_slice(obj);
        }
        offsets.push(pdf.len());
        pdf.extend_from_slice(obj4_head.as_bytes());
        pdf.resize(pdf.len() + extra_stream_bytes, b' ');
        pdf.extend_from_slice(obj4_tail);

        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for off in &offsets[1..] {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        pdf
    }

    fn sample_n_page_pdf(page_count: usize) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut kids = String::new();
        for idx in 0..page_count {
            if idx > 0 {
                kids.push(' ');
            }
            let _ = write!(kids, "{} 0 R", idx + 3);
        }
        let obj1 = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec();
        let obj2 =
            format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {page_count} >>\nendobj\n")
                .into_bytes();

        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0usize];
        offsets.push(pdf.len());
        pdf.extend_from_slice(&obj1);
        offsets.push(pdf.len());
        pdf.extend_from_slice(&obj2);
        for idx in 0..page_count {
            offsets.push(pdf.len());
            let obj = format!(
                "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>\nendobj\n",
                idx + 3
            );
            pdf.extend_from_slice(obj.as_bytes());
        }
        let xref = pdf.len();
        let size = page_count + 3;
        pdf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for off in &offsets[1..] {
            pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n")
                .as_bytes(),
        );
        pdf
    }

    #[test]
    fn pdf_import_allows_pdfs_above_32mib_64mib_and_500_pages() {
        let mut s = Session::default();
        let pdf_34_a = sample_large_pdf(34 * 1024 * 1024);
        assert!(pdf_34_a.len() > 32 * 1024 * 1024);

        // Importing via `pdf.import` command by file path (the UI flow) must succeed > 32 MiB.
        let temp_path =
            std::env::temp_dir().join(format!("drawcraft_big_pdf_{}.pdf", std::process::id()));
        struct RemoveOnDrop(std::path::PathBuf);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        std::fs::write(&temp_path, &pdf_34_a).unwrap();
        let _cleanup = RemoveOnDrop(temp_path.clone());
        let cmd_res = s
            .execute(
                "pdf.import",
                &json!({ "path": temp_path.to_string_lossy(), "mode": "new_notebook", "replace_active": true }),
            )
            .expect("pdf.import command > 32 MiB should succeed");
        assert_eq!(cmd_res["pageCount"], 1);
        assert_eq!(s.doc.pdf_documents.len(), 1);
        assert_eq!(s.doc.pdf_documents[0].bytes.len(), pdf_34_a.len());

        // Inserting a second distinct 34 MiB PDF exceeds 64 MiB combined and must succeed.
        let pdf_34_b = sample_large_pdf(34 * 1024 * 1024 + 17);
        let res_b = s
            .import_pdf_bytes(
                &pdf_34_b,
                Some("Part2.pdf"),
                PdfImportMode::InsertPages,
                false,
            )
            .expect("combined embedded PDFs > 64 MiB should import cleanly");
        assert_eq!(res_b["insertedPages"], 1);
        assert_eq!(s.doc.pdf_documents.len(), 2);
        assert_eq!(s.doc.page_count(), 2);

        // Importing a 550-page PDF must succeed and allow adding/duplicating more pages.
        let pdf_550 = sample_n_page_pdf(550);
        let res_550 = s
            .import_pdf_bytes(
                &pdf_550,
                Some("LongManual.pdf"),
                PdfImportMode::NewNotebook,
                false,
            )
            .expect("550-page PDF should import without 500-page limit");
        assert_eq!(res_550["pageCount"], 550);
        assert_eq!(s.doc.page_count(), 550);
        s.execute("page.add", &Value::Null).unwrap();
        assert_eq!(s.doc.page_count(), 551);
        s.execute("page.duplicate", &Value::Null).unwrap();
        assert_eq!(s.doc.page_count(), 552);

        // Binary .ncraft round-trip preserves all 552 pages.
        let saved = drawcraft_format::save_document_bytes(&s.doc).unwrap();
        let reloaded = drawcraft_format::load_document_bytes(&saved).unwrap();
        assert_eq!(reloaded.page_count(), 552);
        assert_eq!(reloaded.pdf_documents.len(), 1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn live_stroke_and_undo_redo_lifecycle() {
        let mut s = Session::default();
        s.begin_live_stroke(StrokeSample::from_xy_pressure(20.0, 20.0, 0.3));
        s.update_live_stroke(StrokeSample::from_xy_pressure(60.0, 40.0, 0.7));
        s.update_live_stroke(StrokeSample::from_xy_pressure(100.0, 35.0, 0.9));
        let id = s.finish_live_stroke().unwrap();
        assert_eq!(s.doc.strokes.len(), 1);
        assert_eq!(s.doc.strokes[0].id, id);
        assert!(s.history.can_undo());

        let u = s.execute("edit.undo", &Value::Null).unwrap();
        assert_eq!(u["undone"], true);
        assert!(s.doc.strokes.is_empty());

        let r = s.execute("edit.redo", &Value::Null).unwrap();
        assert_eq!(r["redone"], true);
        assert_eq!(s.doc.strokes.len(), 1);
    }

    #[test]
    fn live_eraser_records_single_undo_only_when_modified() {
        let mut s = Session::default();
        s.commit_raw_stroke(
            PenSettings::ball_pen(),
            vec![
                StrokeSample::from_xy_pressure(100.0, 100.0, 0.8),
                StrokeSample::from_xy_pressure(200.0, 100.0, 0.8),
            ],
        );
        // Miss the stroke
        s.begin_live_eraser(Point::new(500.0, 500.0));
        assert!(!s.finish_live_eraser());
        assert_eq!(s.doc.strokes.len(), 1);

        // Hit the stroke
        s.begin_live_eraser(Point::new(150.0, 100.0));
        assert!(s.finish_live_eraser());
        assert!(s.doc.strokes.is_empty());

        // Undo restores the erased stroke in one step
        s.execute("edit.undo", &Value::Null).unwrap();
        assert_eq!(s.doc.strokes.len(), 1);
    }

    #[test]
    fn apply_pen_and_stabilization_presets_update_session() {
        let mut s = Session::default();
        let custom_color = drawcraft_color::Color::from_rgba8(0xAA, 0x11, 0x22, 0xFF);
        s.execute(
            "pen.setSettings",
            &serde_json::json!({ "kind": "BrushPen", "color": custom_color }),
        )
        .unwrap();

        let res = s
            .execute(
                "pen.applyPreset",
                &serde_json::json!({ "preset": "BrushSoftWash", "preserve_color": true }),
            )
            .unwrap();
        assert_eq!(res["kind"], "BrushPen");
        assert_eq!(s.active_tool, ToolKind::Pen(PenKind::BrushPen));
        assert_eq!(s.pen(PenKind::BrushPen).color, custom_color);
        assert!(
            (s.pen(PenKind::BrushPen).size - 22.0).abs() < 1e-4,
            "expected BrushSoftWash size 22.0, got {}",
            s.pen(PenKind::BrushPen).size
        );

        s.execute(
            "pen.setSmoothing",
            &serde_json::json!({
                "kind": "BrushPen",
                "stabilizer_mode": "CircularBuffer",
                "buffer_averaging": "ArithmeticMean",
                "buffer_preprocessor": "Deadzone",
                "buffer_size": 10,
                "buffer_deadzone_radius": 1.3,
                "buffer_cusp_detection": true,
                "buffer_finalize_stroke": true
            }),
        )
        .unwrap();
        assert_eq!(
            s.pen(PenKind::BrushPen).smoothing.stabilizer_mode,
            drawcraft_doc::StabilizerMode::CircularBuffer
        );
        assert_eq!(
            s.pen(PenKind::BrushPen).smoothing.buffer_averaging,
            drawcraft_doc::BufferAveraging::ArithmeticMean
        );
        assert_eq!(
            s.pen(PenKind::BrushPen).smoothing.buffer_preprocessor,
            drawcraft_doc::BufferPreprocessor::Deadzone
        );
        assert_eq!(s.pen(PenKind::BrushPen).smoothing.buffer_size, 10);

        for &preset in drawcraft_doc::StabilizationPreset::ALL {
            let p_res = s
                .execute(
                    "pen.setSmoothing",
                    &serde_json::json!({
                        "kind": "FountainPen",
                        "preset": preset,
                    }),
                )
                .unwrap();
            assert_eq!(s.pen(PenKind::FountainPen).smoothing.preset(), preset);
            assert_eq!(p_res["preset"], serde_json::to_value(preset).unwrap());
        }
    }

    #[test]
    fn notebook_library_pages_and_multi_format_export() {
        let mut s = Session::default();
        s.commit_raw_stroke(
            PenSettings::fountain_pen(),
            vec![
                StrokeSample::from_xy_pressure(50.0, 50.0, 0.5),
                StrokeSample::from_xy_pressure(120.0, 80.0, 0.8),
            ],
        );

        let added = s.execute("page.add", &Value::Null).unwrap();
        assert_eq!(added["pageCount"], 2);
        assert_eq!(added["pageIndex"], 1);
        assert!(s.doc.strokes.is_empty());

        s.execute("page.select", &serde_json::json!({ "index": 0 }))
            .unwrap();
        assert_eq!(s.doc.strokes.len(), 1);

        let created = s
            .execute(
                "notebook.create",
                &serde_json::json!({ "preset": "EngineeringGrid", "title": "Physics Lab" }),
            )
            .unwrap();
        assert_eq!(created["notebookCount"], 2);
        assert_eq!(s.doc.title, "Physics Lab");

        let png = s
            .execute("export.png", &serde_json::json!({ "in_memory": true }))
            .unwrap();
        assert!(png["bytes"].as_u64().unwrap() > 100);

        let svg = s
            .execute("export.svg", &serde_json::json!({ "in_memory": true }))
            .unwrap();
        assert!(svg["svg"].as_str().unwrap().contains("<svg"));

        let pdf = s
            .execute("export.pdf", &serde_json::json!({ "in_memory": true }))
            .unwrap();
        assert!(pdf["bytes"].as_u64().unwrap() > 100);

        let saved = s
            .execute("notebook.save", &serde_json::json!({ "in_memory": true }))
            .unwrap();
        let raw_json = saved["json"].as_str().unwrap().to_string();

        s.execute("notebook.select", &serde_json::json!({ "index": 0 }))
            .unwrap();
        assert_eq!(s.doc.page_count(), 2);

        let opened = s
            .execute("notebook.load", &serde_json::json!({ "json": raw_json }))
            .unwrap();
        assert_eq!(opened["title"], "Physics Lab");
        assert_eq!(s.notebooks.len(), 3);

        let legacy_json = r#"{
            "width": 800,
            "height": 1000,
            "paper_template": "Blank",
            "paper_color": "PureWhite",
            "strokes": [],
            "next_stroke_id": 1
        }"#;
        let legacy: Document = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.title, "Quick Notes");
        assert_eq!(legacy.page_count(), 1);
    }

    #[test]
    fn highlighter_mode_toggles_between_freehand_and_straight_lines() {
        let mut s = Session::default();
        assert_eq!(s.highlighter_mode, HighlighterMode::Freehand);
        s.execute("tool.select", &serde_json::json!({ "tool": "Highlighter" }))
            .unwrap();

        // Freehand mode preserves curved excursion between endpoints.
        s.begin_live_stroke(StrokeSample::from_xy_pressure(100.0, 200.0, 0.5));
        for step in 1..=10 {
            let t = step as f64 / 10.0;
            let x = 100.0 + t * 100.0;
            let y = 200.0 - 80.0 * (std::f64::consts::PI * t).sin();
            s.update_live_stroke(StrokeSample::from_xy_pressure(x, y, 0.5));
        }
        let free_id = s.finish_live_stroke().unwrap();
        let free_stroke = s.doc.strokes.iter().find(|st| st.id == free_id).unwrap();
        let max_y_dev_free = free_stroke
            .smoothed_points
            .iter()
            .map(|p| (p.y - 200.0).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            max_y_dev_free > 10.0,
            "expected freehand highlight to preserve vertical bend, got max deviation {max_y_dev_free}"
        );

        // Straight mode snaps both live preview and committed stroke to start -> end line.
        let res = s
            .execute(
                "highlighter.setSettings",
                &serde_json::json!({ "mode": "Straight" }),
            )
            .unwrap();
        assert_eq!(res["mode"], "Straight");
        assert_eq!(s.highlighter_mode, HighlighterMode::Straight);

        s.begin_live_stroke(StrokeSample::from_xy_pressure(100.0, 300.0, 0.5));
        s.update_live_stroke(StrokeSample::from_xy_pressure(150.0, 240.0, 0.5));
        s.update_live_stroke(StrokeSample::from_xy_pressure(220.0, 300.0, 0.5));
        let preview = s.live_stroke.as_ref().unwrap().preview_samples();
        assert!(preview.iter().all(|p| (p.y - 300.0).abs() < 1e-6));

        let straight_id = s.finish_live_stroke().unwrap();
        let straight_stroke = s
            .doc
            .strokes
            .iter()
            .find(|st| st.id == straight_id)
            .unwrap();
        assert!(
            straight_stroke
                .smoothed_points
                .iter()
                .all(|p| (p.y - 300.0).abs() < 1e-6),
            "expected straight highlight points to stay collinear at y=300"
        );
        assert_eq!(straight_stroke.smoothed_points[0].x, 100.0);
        assert_eq!(straight_stroke.smoothed_points.last().unwrap().x, 220.0);

        // Command-driven stroke.draw honors Straight mode for Highlighter only.
        s.execute(
            "stroke.draw",
            &serde_json::json!({
                "kind": "Highlighter",
                "points": [
                    { "x": 50.0, "y": 400.0, "pressure": 0.5, "time_ms": 0.0 },
                    { "x": 120.0, "y": 350.0, "pressure": 0.5, "time_ms": 16.0 },
                    { "x": 210.0, "y": 400.0, "pressure": 0.5, "time_ms": 32.0 }
                ]
            }),
        )
        .unwrap();
        let cmd_stroke = s.doc.strokes.last().unwrap();
        assert!(
            cmd_stroke
                .smoothed_points
                .iter()
                .all(|p| (p.y - 400.0).abs() < 1e-6)
        );
    }

    #[test]
    fn image_commands_insert_transform_bmp_alpha_fix_delete_and_undo() {
        let mut s = Session::default();
        let png = drawcraft_raster::png_encode(4, 2, &[200u8; 4 * 2 * 4]).unwrap();
        let res = s
            .execute("image.insert", &serde_json::json!({ "bytes": png }))
            .unwrap();
        assert_eq!(res["imageIndex"], 0);
        assert_eq!(res["imageCount"], 1);
        assert_eq!(s.doc.images.len(), 1);
        assert_eq!(s.doc.images[0].pixel_width, 4);
        assert_eq!(s.doc.images[0].pixel_height, 2);

        // Update position & rotation
        s.execute(
            "image.update",
            &serde_json::json!({ "index": 0, "x": 50.0, "y": 60.0, "angle_degrees": 45.0 }),
        )
        .unwrap();
        assert_eq!(s.doc.images[0].x, 50.0);
        assert_eq!(s.doc.images[0].angle_degrees, 45.0);

        // Selection transform
        let pivot = s.doc.images[0].center();
        s.execute(
            "selection.transform",
            &serde_json::json!({
                "targets": [{ "kind": "image", "index": 0 }],
                "pivot": { "x": pivot.x, "y": pivot.y },
                "translation": { "x": 10.0, "y": -5.0 },
                "scale": 2.0,
                "rotation_degrees": 15.0
            }),
        )
        .unwrap();
        assert!((s.doc.images[0].angle_degrees - 60.0).abs() < 1e-6);

        // Base64 insert + 32-bit zero-alpha BMP normalization
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&58u32.to_le_bytes()); // file size
        bmp.extend_from_slice(&0u32.to_le_bytes()); // reserved
        bmp.extend_from_slice(&54u32.to_le_bytes()); // pixel offset
        bmp.extend_from_slice(&40u32.to_le_bytes()); // DIB size
        bmp.extend_from_slice(&1i32.to_le_bytes()); // width
        bmp.extend_from_slice(&1i32.to_le_bytes()); // height
        bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
        bmp.extend_from_slice(&32u16.to_le_bytes()); // bpp
        bmp.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        bmp.extend_from_slice(&4u32.to_le_bytes()); // size image
        bmp.extend_from_slice(&[0i32.to_le_bytes(); 4].concat()); // ppm + colors
        bmp.extend_from_slice(&[10u8, 20u8, 200u8, 0u8]); // BGRA with zero alpha

        let b64 = drawcraft_doc::base64_encode_bytes(&bmp);
        let bmp_res = s
            .execute("image.insert", &serde_json::json!({ "base64_bytes": b64 }))
            .unwrap();
        assert_eq!(bmp_res["imageIndex"], 1);
        let decoded_bmp = image::load_from_memory(&s.doc.images[1].png_bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded_bmp.get_pixel(0, 0).0, [200, 20, 10, 255]);

        // Selection delete + undo
        s.execute(
            "selection.delete",
            &serde_json::json!({ "targets": [{ "kind": "image", "index": 0 }] }),
        )
        .unwrap();
        assert_eq!(s.doc.images.len(), 1);
        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert_eq!(s.doc.images.len(), 2);

        // Clear page clears images too
        let clr = s.execute("page.clear", &serde_json::json!({})).unwrap();
        assert_eq!(clr["clearedImages"], 2);
        assert!(s.doc.images.is_empty());
    }

    #[test]
    fn text_box_max_width_persists_through_updates_scaling_and_undo() {
        let mut s = Session::default();
        s.execute(
            "text.insert",
            &serde_json::json!({
                "text": "Resizable paragraph text",
                "x": 100.0,
                "y": 150.0,
                "size": 24.0,
            }),
        )
        .unwrap();
        assert_eq!(s.doc.texts[0].max_width, None);

        // First width-drag step records history; second with record_history: false coalesces.
        s.execute(
            "text.update",
            &serde_json::json!({
                "index": 0,
                "x": 90.0,
                "max_width": 220.0,
                "record_history": true,
            }),
        )
        .unwrap();
        s.execute(
            "text.update",
            &serde_json::json!({
                "index": 0,
                "x": 80.0,
                "max_width": 230.0,
                "record_history": false,
            }),
        )
        .unwrap();
        assert_eq!(s.doc.texts[0].x, 80.0);
        assert_eq!(s.doc.texts[0].max_width, Some(230.0));

        // Style/content edits without max_width preserve the established box width.
        s.execute(
            "text.update",
            &serde_json::json!({
                "index": 0,
                "text": "Edited longer paragraph text",
                "size": 20.0,
            }),
        )
        .unwrap();
        assert_eq!(s.doc.texts[0].max_width, Some(230.0));
        assert_eq!(s.doc.texts[0].size, 20.0);

        // Corner scaling via selection.transform scales size and max_width proportionally.
        s.execute(
            "selection.transform",
            &serde_json::json!({
                "targets": [{ "kind": "text", "index": 0 }],
                "pivot": { "x": 80.0, "y": 150.0 },
                "translation": { "x": 0.0, "y": 0.0 },
                "scale": 1.5,
                "rotation_degrees": 0.0,
            }),
        )
        .unwrap();
        assert_eq!(s.doc.texts[0].size, 30.0);
        assert_eq!(s.doc.texts[0].max_width, Some(345.0));

        // Undo reverts transform, then style edit, then entire width drag in one step.
        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert_eq!(s.doc.texts[0].max_width, Some(230.0));
        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert_eq!(s.doc.texts[0].text, "Resizable paragraph text");
        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert_eq!(s.doc.texts[0].x, 100.0);
        assert_eq!(s.doc.texts[0].max_width, None);
    }

    #[test]
    fn text_link_url_commands_validate_update_clear_and_undo() {
        let mut s = Session::default();
        assert!(
            s.execute(
                "text.insert",
                &serde_json::json!({
                    "text": "Bad link",
                    "x": 50.0,
                    "y": 60.0,
                    "link_url": "javascript:alert(1)"
                }),
            )
            .is_err()
        );

        let inserted = s
            .execute(
                "text.insert",
                &serde_json::json!({
                    "text": "Docs",
                    "x": 50.0,
                    "y": 60.0,
                    "link_url": "docs.rs/egui"
                }),
            )
            .unwrap();
        assert_eq!(inserted["textIndex"], 0);
        assert_eq!(
            s.doc.texts[0].link_url.as_deref(),
            Some("https://docs.rs/egui")
        );

        s.execute(
            "text.update",
            &serde_json::json!({
                "index": 0,
                "clear_link_url": true,
            }),
        )
        .unwrap();
        assert_eq!(s.doc.texts[0].link_url, None);

        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert_eq!(
            s.doc.texts[0].link_url.as_deref(),
            Some("https://docs.rs/egui")
        );
    }

    #[test]
    fn selection_transform_supports_independent_scale_x_and_scale_y_with_undo() {
        let mut s = Session::default();
        let png = drawcraft_raster::png_encode(4, 2, &[180u8; 4 * 2 * 4]).unwrap();
        s.execute(
            "image.insert",
            &serde_json::json!({ "bytes": png, "x": 100.0, "y": 100.0, "width": 80.0 }),
        )
        .unwrap();
        s.execute(
            "shape.insert",
            &serde_json::json!({
                "kind": "Rectangle",
                "x1": 100.0,
                "y1": 100.0,
                "x2": 200.0,
                "y2": 160.0,
            }),
        )
        .unwrap();

        let img_before = s.doc.images[0].as_ref().clone();
        s.execute(
            "selection.transform",
            &serde_json::json!({
                "targets": [
                    { "kind": "image", "index": 0 },
                    { "kind": "shape", "index": 0 },
                ],
                "pivot": { "x": 100.0, "y": 100.0 },
                "translation": { "x": 0.0, "y": 0.0 },
                "scale_x": 2.0,
                "scale_y": 0.5,
                "rotation_degrees": 0.0,
            }),
        )
        .unwrap();

        assert!((s.doc.images[0].width - img_before.width * 2.0).abs() < 1e-4);
        assert!((s.doc.images[0].height - img_before.height * 0.5).abs() < 1e-4);
        assert!((s.doc.shapes[0].x1 - 100.0).abs() < 1e-6);
        assert!((s.doc.shapes[0].x2 - 300.0).abs() < 1e-6);
        assert!((s.doc.shapes[0].y1 - 100.0).abs() < 1e-6);
        assert!((s.doc.shapes[0].y2 - 130.0).abs() < 1e-6);

        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert!((s.doc.images[0].width - img_before.width).abs() < 1e-6);
        assert!((s.doc.images[0].height - img_before.height).abs() < 1e-6);
        assert!((s.doc.shapes[0].x2 - 200.0).abs() < 1e-6);
        assert!((s.doc.shapes[0].y2 - 160.0).abs() < 1e-6);
    }

    #[test]
    fn image_insert_supports_webp_gif_tiff_tga_and_exif_orientation() {
        let mut s = Session::default();
        let mut rgba = image::RgbaImage::new(3, 2);
        rgba.put_pixel(0, 0, image::Rgba([240, 20, 20, 255]));
        rgba.put_pixel(2, 1, image::Rgba([20, 40, 240, 255]));
        let dyn_img = image::DynamicImage::ImageRgba8(rgba);

        for format in [
            image::ImageFormat::WebP,
            image::ImageFormat::Gif,
            image::ImageFormat::Tiff,
            image::ImageFormat::Tga,
        ] {
            let mut buf = Vec::new();
            dyn_img
                .write_to(&mut std::io::Cursor::new(&mut buf), format)
                .unwrap();
            let res = s
                .execute("image.insert", &serde_json::json!({ "bytes": buf }))
                .unwrap();
            let idx = res["imageIndex"].as_u64().unwrap() as usize;
            assert_eq!(s.doc.images[idx].pixel_width, 3);
            assert_eq!(s.doc.images[idx].pixel_height, 2);
        }

        // JPEG with EXIF Orientation=6 (Rotate 90 CW) should transpose 4x2 -> 2x4.
        let rect = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            2,
            image::Rgba([10, 180, 90, 255]),
        ));
        let mut jpeg = Vec::new();
        rect.write_to(
            &mut std::io::Cursor::new(&mut jpeg),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
        let mut jpeg_with_exif = Vec::with_capacity(jpeg.len() + 36);
        jpeg_with_exif.extend_from_slice(&jpeg[..2]); // SOI FF D8
        jpeg_with_exif.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x22]); // APP1 len=34
        jpeg_with_exif.extend_from_slice(b"Exif\0\0");
        jpeg_with_exif.extend_from_slice(b"II"); // little-endian TIFF
        jpeg_with_exif.extend_from_slice(&42u16.to_le_bytes());
        jpeg_with_exif.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        jpeg_with_exif.extend_from_slice(&1u16.to_le_bytes()); // 1 IFD entry
        jpeg_with_exif.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation tag
        jpeg_with_exif.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        jpeg_with_exif.extend_from_slice(&1u32.to_le_bytes()); // count
        jpeg_with_exif.extend_from_slice(&6u32.to_le_bytes()); // Rotate90
        jpeg_with_exif.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        jpeg_with_exif.extend_from_slice(&jpeg[2..]);

        let exif_res = s
            .execute(
                "image.insert",
                &serde_json::json!({ "bytes": jpeg_with_exif }),
            )
            .unwrap();
        let exif_idx = exif_res["imageIndex"].as_u64().unwrap() as usize;
        assert_eq!(s.doc.images[exif_idx].pixel_width, 2);
        assert_eq!(s.doc.images[exif_idx].pixel_height, 4);
    }

    #[test]
    fn page_management_commands_support_move_bookmark_rotate_batch_transfer_and_merge() {
        use drawcraft_doc::PaperTemplate;

        let mut s = Session::default();
        s.execute("page.add", &serde_json::json!({ "template": "Grid" }))
            .unwrap();
        s.execute("page.add", &serde_json::json!({ "template": "Checklist" }))
            .unwrap();
        assert_eq!(s.doc.page_count(), 3);

        // Bookmark page 0 and page 2
        s.execute(
            "page.setBookmark",
            &serde_json::json!({ "index": 0, "bookmarked": true }),
        )
        .unwrap();
        s.execute("page.setBookmark", &serde_json::json!({ "index": 2 }))
            .unwrap();
        assert!(s.doc.pages[0].bookmarked);
        assert!(s.doc.pages[2].bookmarked);

        // Move active page 2 to index 0
        s.execute("page.move", &serde_json::json!({ "from": 2, "to": 0 }))
            .unwrap();
        assert_eq!(s.doc.active_page_index(), 0);
        assert_eq!(s.doc.paper_template, PaperTemplate::Checklist);

        // Rotate batch [0, 1] by 90 CW
        let old_w = s.doc.width;
        let old_h = s.doc.height;
        s.execute(
            "page.rotate",
            &serde_json::json!({ "pages": [0, 1], "quarter_turns": 1 }),
        )
        .unwrap();
        assert_eq!((s.doc.width, s.doc.height), (old_h, old_w));
        assert_eq!(
            (s.doc.pages[1].width, s.doc.pages[1].height),
            (old_h, old_w)
        );

        // Apply template across all pages
        s.execute(
            "page.applyTemplateAll",
            &serde_json::json!({ "template": "Isometric", "color": "PureWhite" }),
        )
        .unwrap();
        assert!(
            s.doc
                .pages
                .iter()
                .all(|p| p.paper_template == PaperTemplate::Isometric)
        );

        // Batch duplicate [0, 2] -> 5 pages; batch remove [1, 3] -> 3 pages
        s.execute(
            "page.batchDuplicate",
            &serde_json::json!({ "indices": [0, 2] }),
        )
        .unwrap();
        assert_eq!(s.doc.page_count(), 5);
        s.execute(
            "page.batchRemove",
            &serde_json::json!({ "indices": [1, 3] }),
        )
        .unwrap();
        assert_eq!(s.doc.page_count(), 3);

        // Partial PDF export of pages [0, 2]
        let exp = s
            .execute(
                "export.pdf",
                &serde_json::json!({ "pages": [0, 2], "in_memory": true }),
            )
            .unwrap();
        assert_eq!(exp["pageCount"], 2);

        // Move page 1 into a new extracted notebook
        let xfer = s
            .execute(
                "page.transfer",
                &serde_json::json!({
                    "pages": [1],
                    "new_notebook_title": "Extracted",
                    "move_pages": true,
                }),
            )
            .unwrap();
        assert_eq!(xfer["transferredCount"], 1);
        assert_eq!(s.doc.page_count(), 2);
        assert_eq!(s.notebooks.len(), 2);
        let target_idx = xfer["targetNotebookIndex"].as_u64().unwrap() as usize;
        assert_eq!(s.notebooks[target_idx].title, "Extracted");

        // Save PDF page as reusable template and apply via notebook.merge test too
        let pdf_bytes = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>\nendobj\n4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Resources << >> >>\nendobj\nxref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000123 00000 n \n0000000210 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n297\n%%EOF\n".to_vec();
        let pdf_tpl = s
            .execute(
                "template.saveFromPdf",
                &serde_json::json!({
                    "name": "Lecture Backdrop",
                    "bytes": pdf_bytes,
                    "page_index": 1,
                }),
            )
            .unwrap();
        let pdf_tpl_id = pdf_tpl["id"].as_str().unwrap().to_string();
        let nb2 = s
            .execute(
                "notebook.create",
                &serde_json::json!({
                    "title": "Merged Source",
                    "custom_template_id": pdf_tpl_id,
                }),
            )
            .unwrap();
        let nb2_idx = nb2["notebookIndex"].as_u64().unwrap() as usize;
        assert!(s.notebooks[nb2_idx].pdf_backdrop.is_some());

        // Merge notebook 2 into notebook 0 and trash source
        let merge = s
            .execute(
                "notebook.merge",
                &serde_json::json!({
                    "source_index": nb2_idx,
                    "target_index": 0,
                    "delete_source": true,
                }),
            )
            .unwrap();
        assert_eq!(merge["mergedPages"], 1);
        assert_eq!(merge["deletedSource"], true);
        assert_eq!(s.notebooks[0].page_count(), 3);
        assert_eq!(s.notebooks[0].pdf_documents.len(), 1);
    }

    #[test]
    fn notebook_create_supports_resolution_presets_and_custom_dimensions() {
        let mut s = Session::default();
        let res = s
            .execute(
                "notebook.create",
                &serde_json::json!({
                    "title": "Slides",
                    "resolution": drawcraft_doc::PageResolutionPreset::Widescreen,
                }),
            )
            .unwrap();
        assert_eq!(res["width"], 1920);
        assert_eq!(res["height"], 1080);
        assert_eq!(s.doc.width, 1920);
        assert_eq!(s.doc.height, 1080);
        assert_eq!(s.doc.page_dimensions(0), Some((1920, 1080)));

        // Adding a page inherits the selected notebook resolution.
        s.execute("page.add", &serde_json::json!({})).unwrap();
        assert_eq!(s.doc.page_dimensions(1), Some((1920, 1080)));

        // Custom dimensions override defaults and reject hostile bounds.
        let custom = s
            .execute(
                "notebook.create",
                &serde_json::json!({
                    "title": "Custom Board",
                    "resolution": drawcraft_doc::PageResolutionPreset::Custom,
                    "width": 2400,
                    "height": 1350,
                }),
            )
            .unwrap();
        assert_eq!(custom["width"], 2400);
        assert_eq!(custom["height"], 1350);
        assert_eq!(s.doc.page_dimensions(0), Some((2400, 1350)));

        assert!(
            s.execute(
                "notebook.create",
                &serde_json::json!({ "width": 32, "height": 1200 })
            )
            .is_err()
        );
        assert!(
            s.execute(
                "notebook.create",
                &serde_json::json!({ "width": 1200, "height": 99999 })
            )
            .is_err()
        );

        // Notebook creation also supports explicit orientation overrides (e.g. A4 Landscape).
        let landscape_a4 = s
            .execute(
                "notebook.create",
                &serde_json::json!({
                    "title": "A4 Landscape Pad",
                    "resolution": drawcraft_doc::PageResolutionPreset::A4,
                    "orientation": drawcraft_doc::PageOrientation::Landscape,
                }),
            )
            .unwrap();
        assert_eq!(landscape_a4["width"], 1684);
        assert_eq!(landscape_a4["height"], 1190);
    }

    #[test]
    fn page_set_size_and_page_add_support_presets_orientation_and_undo() {
        let mut s = Session::default();
        assert_eq!(s.doc.page_dimensions(0), Some((1200, 1600)));

        // Add page 2 using A4 Landscape + Grid template.
        let added = s
            .execute(
                "page.add",
                &serde_json::json!({
                    "template": drawcraft_doc::PaperTemplate::Grid,
                    "resolution": drawcraft_doc::PageResolutionPreset::A4,
                    "orientation": drawcraft_doc::PageOrientation::Landscape,
                }),
            )
            .unwrap();
        assert_eq!(added["pageIndex"], 1);
        assert_eq!(added["width"], 1684);
        assert_eq!(added["height"], 1190);
        assert_eq!(s.doc.page_dimensions(0), Some((1200, 1600)));
        assert_eq!(s.doc.page_dimensions(1), Some((1684, 1190)));

        // Resize only the active page (page 1) to US Letter Portrait.
        let resized = s
            .execute(
                "page.setSize",
                &serde_json::json!({
                    "resolution": drawcraft_doc::PageResolutionPreset::UsLetter,
                    "orientation": drawcraft_doc::PageOrientation::Portrait,
                }),
            )
            .unwrap();
        assert_eq!(resized["updatedCount"], 1);
        assert_eq!(resized["width"], 1224);
        assert_eq!(resized["height"], 1584);
        assert_eq!(s.doc.page_dimensions(0), Some((1200, 1600)));
        assert_eq!(s.doc.page_dimensions(1), Some((1224, 1584)));

        // Apply orientation-only change across all pages preserves distinct page sizes.
        let flipped = s
            .execute(
                "page.setSize",
                &serde_json::json!({
                    "apply_all": true,
                    "orientation": drawcraft_doc::PageOrientation::Landscape,
                }),
            )
            .unwrap();
        assert_eq!(flipped["updatedCount"], 2);
        assert_eq!(s.doc.page_dimensions(0), Some((1600, 1200)));
        assert_eq!(s.doc.page_dimensions(1), Some((1584, 1224)));

        // Undo restores the per-page Portrait dimensions before `apply_all`.
        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert_eq!(s.doc.page_dimensions(0), Some((1200, 1600)));
        assert_eq!(s.doc.page_dimensions(1), Some((1224, 1584)));

        // Redo reapplies `apply_all`.
        s.execute("edit.redo", &serde_json::json!({})).unwrap();
        assert_eq!(s.doc.page_dimensions(0), Some((1600, 1200)));
        assert_eq!(s.doc.page_dimensions(1), Some((1584, 1224)));

        // Rejects missing size params or out-of-range dimensions cleanly.
        assert!(s.execute("page.setSize", &serde_json::json!({})).is_err());
        assert!(
            s.execute(
                "page.setSize",
                &serde_json::json!({ "width": 16, "height": 800 })
            )
            .is_err()
        );
        assert!(
            s.execute(
                "page.add",
                &serde_json::json!({ "width": 800, "height": 10000 })
            )
            .is_err()
        );
    }

    #[test]
    fn roadmap_section_4_tools_shapes_and_highlighter_eraser_work() {
        let mut s = Session::default();

        // 1. Pencil & LaserPointer tool selection + Draw & Hold live shape recognition
        s.execute("tool.select", &serde_json::json!({ "tool": "Pencil" }))
            .unwrap();
        assert_eq!(s.active_tool, ToolKind::Pen(PenKind::Pencil));
        s.execute(
            "tool.select",
            &serde_json::json!({ "tool": "LaserPointer" }),
        )
        .unwrap();
        assert_eq!(s.active_tool, ToolKind::LaserPointer);
        s.execute("tool.select", &serde_json::json!({ "tool": "Pencil" }))
            .unwrap();

        let circle_pts: Vec<StrokeSample> = (0..=40)
            .map(|i| {
                let a = std::f64::consts::TAU * (i as f64 / 40.0);
                StrokeSample::from_xy_pressure(240.0 + 60.0 * a.cos(), 240.0 + 60.0 * a.sin(), 0.6)
            })
            .collect();
        s.begin_live_stroke(circle_pts[0]);
        for &pt in circle_pts.iter().skip(1) {
            s.update_live_stroke(pt);
        }
        assert!(s.lock_live_stroke_recognized_shape());
        let finished = s.finish_live_stroke_with_shape();
        assert!(matches!(finished, Some(FinishedLiveDraw::Shape { .. })));
        assert_eq!(s.doc.shapes.len(), 1);
        assert_eq!(s.doc.shapes[0].kind, drawcraft_doc::ShapeKind::Ellipse);

        // 2. Shape fill, dashed border & update command
        s.execute(
            "shape.update",
            &serde_json::json!({
                "index": 0,
                "fill_color": { "r": 0.2, "g": 0.4, "b": 0.9, "a": 0.35 },
                "line_style": "Dashed",
            }),
        )
        .unwrap();
        assert!(s.doc.shapes[0].fill_color.is_some());
        assert_eq!(
            s.doc.shapes[0].line_style,
            drawcraft_doc::ShapeLineStyle::Dashed
        );

        // 3. Highlighter-only eraser filter leaves ink & pencil strokes intact
        s.execute(
            "stroke.draw",
            &serde_json::json!({
                "kind": "Pencil",
                "points": [
                    StrokeSample::from_xy_pressure(100.0, 400.0, 0.7),
                    StrokeSample::from_xy_pressure(300.0, 400.0, 0.7),
                ]
            }),
        )
        .unwrap();
        s.execute(
            "stroke.draw",
            &serde_json::json!({
                "kind": "Highlighter",
                "points": [
                    StrokeSample::from_xy_pressure(100.0, 400.0, 0.7),
                    StrokeSample::from_xy_pressure(300.0, 400.0, 0.7),
                ]
            }),
        )
        .unwrap();
        assert_eq!(s.doc.strokes.len(), 2);

        s.execute(
            "eraser.setSettings",
            &serde_json::json!({ "highlighter_only": true }),
        )
        .unwrap();
        s.execute(
            "stroke.erase",
            &serde_json::json!({
                "mode": "StrokeEraser",
                "path": [{ "x": 200.0, "y": 400.0, "radius": 24.0 }]
            }),
        )
        .unwrap();
        assert_eq!(s.doc.strokes.len(), 1);
        assert_eq!(s.doc.strokes[0].pen.kind, PenKind::Pencil);
    }

    #[test]
    fn roadmap_section_4_selection_rich_text_elements_and_favorites_work() {
        let mut s = Session::default();
        s.execute(
            "shape.insert",
            &serde_json::json!({ "kind": "Ellipse", "x1": 180.0, "y1": 180.0, "x2": 300.0, "y2": 300.0 }),
        )
        .unwrap();
        s.execute(
            "stroke.draw",
            &serde_json::json!({
                "kind": "Pencil",
                "points": [
                    StrokeSample::from_xy_pressure(100.0, 400.0, 0.7),
                    StrokeSample::from_xy_pressure(300.0, 400.0, 0.7),
                ]
            }),
        )
        .unwrap();

        // Rich text insert + toggleCheckbox + duplicate/copy/paste across notebooks
        s.execute(
            "text.insert",
            &serde_json::json!({
                "text": "[ ] Task A\nDetails",
                "x": 120.0,
                "y": 500.0,
                "alignment": "Center",
                "spans": [{ "start": 4, "end": 10, "bold": true, "italic": false, "underline": true }]
            }),
        )
        .unwrap();
        s.execute(
            "text.toggleCheckbox",
            &serde_json::json!({ "index": 0, "paragraph_index": 0 }),
        )
        .unwrap();
        assert!(s.doc.texts[0].text.starts_with("[x] Task A"));

        let targets = serde_json::json!([
            { "kind": "stroke", "id": s.doc.strokes[0].id.0 },
            { "kind": "shape", "index": 0 },
            { "kind": "text", "index": 0 },
        ]);
        s.execute(
            "selection.duplicate",
            &serde_json::json!({ "targets": targets.clone() }),
        )
        .unwrap();
        assert_eq!(s.doc.strokes.len(), 2);
        assert_eq!(s.doc.shapes.len(), 2);
        assert_eq!(s.doc.texts.len(), 2);

        s.execute("selection.copy", &serde_json::json!({ "targets": targets }))
            .unwrap();
        s.execute(
            "notebook.create",
            &serde_json::json!({ "title": "Target Pad" }),
        )
        .unwrap();
        assert_eq!(s.doc.strokes.len(), 0);
        s.execute("selection.paste", &serde_json::json!({}))
            .unwrap();
        assert_eq!(s.doc.strokes.len(), 1);
        assert_eq!(s.doc.shapes.len(), 1);
        assert_eq!(s.doc.texts.len(), 1);

        // 5. Stickers / Elements (built-in AI image stickers + user-saved) + Style favorites
        let builtin_res = s
            .execute(
                "element.place",
                &serde_json::json!({ "id": "builtin_study_cat", "x": 300.0, "y": 300.0 }),
            )
            .unwrap();
        assert_eq!(builtin_res["image_indices"], serde_json::json!([0]));
        assert_eq!(s.doc.images.len(), 1);
        let decoded = image::load_from_memory(&s.doc.images[0].png_bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (320, 320));
        assert_eq!(decoded.get_pixel(0, 0).0[3], 0);
        assert!(decoded.pixels().any(|p| p.0[3] == 255));

        let elem_save = s
            .execute(
                "element.save",
                &serde_json::json!({
                    "name": "My Diagram",
                    "targets": [{ "kind": "shape", "index": 0 }, { "kind": "text", "index": 0 }, { "kind": "image", "index": 0 }]
                }),
            )
            .unwrap();
        let el_id = elem_save["element"]["id"].as_str().unwrap().to_string();
        s.execute("page.add", &serde_json::json!({})).unwrap();
        s.execute(
            "element.place",
            &serde_json::json!({ "id": el_id, "x": 400.0, "y": 400.0 }),
        )
        .unwrap();
        assert_eq!(s.doc.shapes.len(), 1);
        assert_eq!(s.doc.texts.len(), 1);
        assert_eq!(s.doc.images.len(), 1);
        s.execute("element.delete", &serde_json::json!({ "id": el_id }))
            .unwrap();
        assert!(s.prefs.saved_elements.is_empty());

        let fav_res = s
            .execute(
                "styleFavorite.save",
                &serde_json::json!({
                    "kind": "Pencil",
                    "size": 5.5,
                    "color": { "r": 0.1, "g": 0.2, "b": 0.7, "a": 1.0 }
                }),
            )
            .unwrap();
        let fav_id = fav_res["favorite"]["id"].as_str().unwrap().to_string();
        s.execute("styleFavorite.apply", &serde_json::json!({ "id": fav_id }))
            .unwrap();
        assert_eq!(s.active_tool, ToolKind::Pen(PenKind::Pencil));
        assert!((s.pen(PenKind::Pencil).size - 5.5).abs() < 1e-4);
        s.execute("styleFavorite.delete", &serde_json::json!({ "id": fav_id }))
            .unwrap();
        assert!(s.prefs.style_favorites.is_empty());
    }

    #[test]
    fn session_remembers_last_pen_kind_across_non_pen_tools() {
        let mut s = Session::default();
        assert_eq!(s.active_tool, ToolKind::Pen(PenKind::FountainPen));
        assert_eq!(s.last_pen_kind, PenKind::FountainPen);

        s.execute("tool.select", &serde_json::json!({ "tool": "BrushPen" }))
            .unwrap();
        assert_eq!(s.last_pen_kind, PenKind::BrushPen);
        assert_eq!(s.active_pen_kind(), PenKind::BrushPen);

        for tool in ["Select", "Eraser", "LaserPointer"] {
            s.execute("tool.select", &serde_json::json!({ "tool": tool }))
                .unwrap();
            assert_eq!(s.last_pen_kind, PenKind::BrushPen);
            assert_eq!(s.active_pen_kind(), PenKind::BrushPen);
        }

        s.execute("tool.select", &serde_json::json!({ "tool": "Pencil" }))
            .unwrap();
        assert_eq!(s.last_pen_kind, PenKind::Pencil);
        s.execute("tool.select", &serde_json::json!({ "tool": "Select" }))
            .unwrap();
        assert_eq!(s.active_pen_kind(), PenKind::Pencil);
    }

    #[test]
    fn stylus_set_buttons_validates_updates_resets_and_preserves_doc_timestamp() {
        let mut s = Session::default();
        let initial_modified = s.doc.updated_at_ms;

        // Empty payload without reset must be rejected.
        assert!(
            s.execute("stylus.setButtons", &serde_json::json!({}))
                .is_err()
        );

        // Non-hold action on eraser_end must be rejected.
        assert!(
            s.execute(
                "stylus.setButtons",
                &serde_json::json!({ "eraser_end": "Undo" })
            )
            .is_err()
        );

        // Partial update changes only requested fields and preserves doc modified timestamp.
        let res = s
            .execute(
                "stylus.setButtons",
                &serde_json::json!({ "barrel_button": "Undo" }),
            )
            .unwrap();
        assert_eq!(res["barrel_button"], "Undo");
        assert_eq!(
            s.prefs.stylus_buttons.barrel_button,
            drawcraft_doc::StylusButtonAction::Undo
        );
        assert_eq!(
            s.prefs.stylus_buttons.secondary_barrel_button,
            drawcraft_doc::StylusButtonAction::PanCanvasHold
        );
        assert_eq!(
            s.prefs.stylus_buttons.eraser_end,
            drawcraft_doc::StylusButtonAction::EraserHold
        );
        assert_eq!(s.doc.updated_at_ms, initial_modified);

        // Full update sets all three slots cleanly.
        s.execute(
            "stylus.setButtons",
            &serde_json::json!({
                "barrel_button": "ToggleSelect",
                "secondary_barrel_button": "Redo",
                "eraser_end": "HighlighterHold",
            }),
        )
        .unwrap();
        assert_eq!(
            s.prefs.stylus_buttons,
            drawcraft_doc::StylusButtonConfig {
                barrel_button: drawcraft_doc::StylusButtonAction::ToggleSelect,
                secondary_barrel_button: drawcraft_doc::StylusButtonAction::Redo,
                eraser_end: drawcraft_doc::StylusButtonAction::HighlighterHold,
            }
        );

        // Reset restores defaults (optionally overriding one field if provided).
        s.execute("stylus.setButtons", &serde_json::json!({ "reset": true }))
            .unwrap();
        assert_eq!(
            s.prefs.stylus_buttons,
            drawcraft_doc::StylusButtonConfig::default()
        );
        assert_eq!(s.doc.updated_at_ms, initial_modified);
    }

    #[test]
    fn default_session_starts_with_blue_fountain_pen() {
        let s = Session::default();
        assert_eq!(s.active_tool, ToolKind::Pen(PenKind::FountainPen));
        assert_eq!(s.active_pen().color, drawcraft_color::INK_PALETTE[1].color);
        assert_eq!(s.active_pen().color.to_hex(), "#0052FF");
    }

    #[test]
    fn selection_transform_uniform_scale_preserves_stroke_and_shape_path_widths() {
        let mut s = Session::default();
        let mut ball_pen = drawcraft_doc::PenSettings::ball_pen();
        ball_pen.size = 2.2;
        let stroke_res = s
            .execute(
                "stroke.draw",
                &serde_json::json!({
                    "pen": ball_pen,
                    "points": [
                        { "x": 100.0, "y": 100.0, "pressure": 0.5 },
                        { "x": 120.0, "y": 135.0, "pressure": 0.5 },
                        { "x": 140.0, "y": 170.0, "pressure": 0.5 },
                        { "x": 160.0, "y": 180.0, "pressure": 0.5 },
                        { "x": 190.0, "y": 150.0, "pressure": 0.5 },
                        { "x": 220.0, "y": 110.0, "pressure": 0.5 },
                    ],
                }),
            )
            .unwrap();
        let stroke_id = stroke_res["strokeId"].as_u64().unwrap();

        s.execute(
            "shape.insert",
            &serde_json::json!({
                "kind": "Rectangle",
                "x1": 100.0,
                "y1": 100.0,
                "x2": 200.0,
                "y2": 160.0,
                "width": 3.5,
                "curve": 12.0,
            }),
        )
        .unwrap();

        let orig_span = drawcraft_geom::Rect::from_point_iter(
            s.doc.strokes[0].smoothed_points.iter().map(|pt| pt.point()),
        );
        s.execute(
            "selection.transform",
            &serde_json::json!({
                "targets": [
                    { "kind": "stroke", "id": stroke_id },
                    { "kind": "shape", "index": 0 },
                ],
                "pivot": { "x": 100.0, "y": 100.0 },
                "translation": { "x": 0.0, "y": 0.0 },
                "scale_x": 3.0,
                "scale_y": 3.0,
                "rotation_degrees": 0.0,
            }),
        )
        .unwrap();

        // Stroke geometry scales 3x while pen thickness stays unchanged.
        let scaled_stroke = &s.doc.strokes[0];
        let scaled_span = drawcraft_geom::Rect::from_point_iter(
            scaled_stroke.smoothed_points.iter().map(|pt| pt.point()),
        );
        assert!(
            (scaled_stroke.pen.size - 2.2).abs() < 1e-5,
            "uniform resize must preserve stroke pen.size (expected 2.2, got {})",
            scaled_stroke.pen.size
        );
        assert!(
            (scaled_span.w - orig_span.w * 3.0).abs() < 1e-3,
            "expected stroke width to scale 3x ({} -> {})",
            orig_span.w,
            scaled_span.w
        );
        assert!(
            (scaled_span.h - orig_span.h * 3.0).abs() < 1e-3,
            "expected stroke height to scale 3x ({} -> {})",
            orig_span.h,
            scaled_span.h
        );

        // Shape geometry scales 3x while outline path width and curve stay unchanged.
        let scaled_shape = &s.doc.shapes[0];
        assert!((scaled_shape.x2 - scaled_shape.x1 - 300.0).abs() < 1e-4);
        assert!((scaled_shape.y2 - scaled_shape.y1 - 180.0).abs() < 1e-4);
        assert!(
            (scaled_shape.width - 3.5).abs() < 1e-5,
            "uniform resize must preserve shape width (expected 3.5, got {})",
            scaled_shape.width
        );
        assert!(
            (scaled_shape.curve - 12.0).abs() < 1e-5,
            "uniform resize must preserve shape curve (expected 12.0, got {})",
            scaled_shape.curve
        );

        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert!((s.doc.strokes[0].pen.size - 2.2).abs() < 1e-5);
        assert!((s.doc.shapes[0].x2 - s.doc.shapes[0].x1 - 100.0).abs() < 1e-4);
    }
}
