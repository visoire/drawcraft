//! Study features of the editor: reveal tapes on the canvas, the flashcard window, study
//! ergonomics preferences (left-handed layout, Pencil-only vs finger drawing, dark PDF pages),
//! and the Cmd/Ctrl+K command palette with Cmd/Ctrl+F notebook search.
//!
//! Every mutation goes through `drawcraft-engine` commands; this module only reads the
//! document to draw it.

use drawcraft_doc::{ReviewGrade, StudyTapeItem};
use drawcraft_geom::{Affine2, Point};
use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Key, Margin, Pos2, Rect, Response, RichText,
    Stroke, StrokeKind, pos2, vec2,
};
use serde_json::json;

use crate::DrawCraftApp;
use crate::icons::{Icon, compact_icon_button};
use crate::theme::Tokens;

/// Smallest drag (screen px) that creates a tape; shorter gestures are taps.
const MIN_TAPE_DRAG_PX: f32 = 12.0;
/// Smallest tape side in page points.
const MIN_TAPE_PAGE_SIDE: f64 = 8.0;
const MAX_PALETTE_ROWS: usize = 12;
const MAX_FIND_RESULTS: usize = 50;
const ROW_HEIGHT: f32 = 44.0;

/// Which tab of the flashcard window is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlashcardTab {
    #[default]
    Review,
    Create,
}

/// UI-only state of the study features (nothing here is persisted or synced).
#[derive(Clone, Debug, Default)]
pub struct StudyUiState {
    /// The Study Tape tool is active: drag to place a tape, tap a tape to reveal or hide it.
    pub tape_mode: bool,
    tape_drag: Option<(Pos2, Pos2)>,
    pub flashcards_open: bool,
    /// Hide all chrome and sidebars; only the page remains.
    pub focus_mode: bool,
    /// The navigation bar's overflow menu is open.
    pub overflow_open: bool,
    /// Settings popover of the active pen, eraser or laser tool (second tap on the tool).
    pub tool_popover_open: bool,
    flashcard_tab: FlashcardTab,
    card_front: String,
    card_back: String,
    card_link_page: bool,
    answer_shown: bool,
    pub palette_open: bool,
    palette_query: String,
    palette_find: bool,
    palette_selected: usize,
    palette_focus: bool,
    /// Single finger currently panning because Pencil-only mode keeps fingers from drawing.
    pub finger_pan: Option<(egui::TouchId, Pos2)>,
    /// A touch screen is in use; controls grow to the 44 px minimum hit target.
    pub touch_ui: bool,
    /// `true` while the browser's latest DOM `PointerEvent` came from a finger (`"touch"`),
    /// blocking `eframe`'s synchronous `pointerdown` frame from drawing before `touchstart` arrives.
    pub pointer_is_direct_touch: bool,
    /// Live touches by DOM identifier with their physical source (Pencil or finger).
    touch_sources: Vec<(i32, crate::TouchSource)>,
    /// Frame in which the brand fonts were handed to egui; the text scale follows one frame later.
    brand_fonts_frame: Option<u64>,
    brand_styles_applied: bool,
    status: Option<String>,
}

/// Keyboard shortcuts owned by this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudyShortcut {
    CommandPalette,
    FindInNotebook,
    FitWidth,
    FitPage,
}

/// Maps Cmd/Ctrl+K, F, 0 and 9 (Magic Keyboard parity) to an action.
#[must_use]
pub fn detect_shortcut(i: &egui::InputState, in_editor: bool) -> Option<StudyShortcut> {
    if !i.modifiers.command || i.modifiers.alt {
        return None;
    }
    if i.key_pressed(Key::K) {
        return Some(StudyShortcut::CommandPalette);
    }
    if !in_editor {
        return None;
    }
    if i.key_pressed(Key::F) {
        Some(StudyShortcut::FindInNotebook)
    } else if i.key_pressed(Key::Num0) {
        Some(StudyShortcut::FitWidth)
    } else if i.key_pressed(Key::Num9) {
        Some(StudyShortcut::FitPage)
    } else {
        None
    }
}

pub(crate) fn execute_shortcut(app: &mut DrawCraftApp, shortcut: StudyShortcut) {
    match shortcut {
        StudyShortcut::CommandPalette => open_palette(app, false),
        StudyShortcut::FindInNotebook => open_palette(app, true),
        StudyShortcut::FitWidth => fit_width(app),
        StudyShortcut::FitPage => app.view.fit_requested = true,
    }
}

fn open_palette(app: &mut DrawCraftApp, find: bool) {
    let s = &mut app.view.study;
    s.palette_open = true;
    s.palette_find = find;
    s.palette_query.clear();
    s.palette_selected = 0;
    s.palette_focus = true;
}

/// Zooms so the page fills the canvas width and scrolls to its top.
pub(crate) fn fit_width(app: &mut DrawCraftApp) {
    let Some(rect) = app.view.last_canvas_rect else {
        return;
    };
    let page_w = f64::from(app.session.doc.width.max(64));
    let pad = 24.0_f64;
    let zoom = ((f64::from(rect.width()) - pad * 2.0) / page_w).clamp(0.15, 4.0);
    let zoom_f = zoom as f32;
    app.view.zoom = zoom_f;
    app.session.zoom = zoom_f;
    app.view.pan = [
        f64::from(rect.center().x) - page_w * zoom * 0.5,
        f64::from(rect.min.y) + pad,
    ];
}

// ---------------------------------------------------------------------------------------------
// Study tapes
// ---------------------------------------------------------------------------------------------

/// Turns the Study Tape tool on or off, leaving every other overlay tool.
pub(crate) fn toggle_tape_mode(app: &mut DrawCraftApp) {
    let on = !app.view.study.tape_mode;
    if on {
        crate::canvas::commit_pending_text(app);
        app.view.text_insert_mode = false;
        app.view.shape_insert_mode = false;
        app.view.shape_drag_start = None;
        app.view.shape_drag_end = None;
        app.view.shape_drag_touch_id = None;
        app.view.ruler_active = false;
        app.view.clear_selection();
        app.view.color_picker.close();
    }
    app.view.study.tape_mode = on;
    app.view.study.tape_drag = None;
}

/// Leaves the tape tool (called whenever another tool is picked).
pub(crate) fn leave_tape_mode(app: &mut DrawCraftApp) {
    app.view.study.tape_mode = false;
    app.view.study.tape_drag = None;
}

fn tape_screen_rect(tape: &StudyTapeItem, xf: Affine2) -> Rect {
    let a = xf.to_screen(Point::new(f64::from(tape.x), f64::from(tape.y)));
    let b = xf.to_screen(Point::new(
        f64::from(tape.x + tape.width),
        f64::from(tape.y + tape.height),
    ));
    Rect::from_two_pos(pos2(a.x as f32, a.y as f32), pos2(b.x as f32, b.y as f32))
}

fn tape_color(tape: &StudyTapeItem) -> Color32 {
    let c = tape.color_rgba;
    Color32::from_rgba_unmultiplied((c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Draws hidden tapes as opaque bars and revealed tapes as a faint outline, plus the drag preview.
pub(crate) fn paint_tapes(app: &DrawCraftApp, p: &egui::Painter, xf: Affine2, t: &Tokens) {
    for tape in &app.session.doc.study_tapes {
        let rect = tape_screen_rect(tape, xf);
        if tape.revealed {
            p.rect_stroke(
                rect,
                CornerRadius::same(3),
                Stroke::new(1.0, t.accent.gamma_multiply(0.6)),
                StrokeKind::Inside,
            );
        } else {
            p.rect_filled(rect, CornerRadius::same(3), tape_color(tape));
            if rect.width() > 96.0 && rect.height() > 22.0 {
                p.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Tap to reveal",
                    FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
        }
    }
    if let Some((a, b)) = app.view.study.tape_drag {
        let rect = Rect::from_two_pos(a, b);
        p.rect_filled(rect, CornerRadius::same(3), t.accent.gamma_multiply(0.35));
        p.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.5, t.accent),
            StrokeKind::Inside,
        );
    }
}

/// Handles drag (place a tape), tap (reveal or hide) and secondary click (remove) while the Study
/// Tape tool is active.
pub(crate) fn handle_tape_input(
    app: &mut DrawCraftApp,
    ui: &egui::Ui,
    resp: &Response,
    xf: Affine2,
) {
    use egui::PointerButton::{Primary, Secondary};
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started_by(Primary) {
        if let Some(pos) = pointer {
            app.view.study.tape_drag = Some((pos, pos));
        }
    } else if resp.dragged_by(Primary)
        && let (Some((start, _)), Some(pos)) = (app.view.study.tape_drag, pointer)
    {
        app.view.study.tape_drag = Some((start, pos));
        ui.ctx().request_repaint();
    }
    if resp.drag_stopped_by(Primary)
        && let Some((a, b)) = app.view.study.tape_drag.take()
        && (a - b).length() >= MIN_TAPE_DRAG_PX
    {
        place_tape(app, xf, a, b);
    }
    if resp.clicked_by(Primary)
        && let Some(pos) = pointer
    {
        if let Some(id) = tape_at(app, xf, pos) {
            run(app, "studyTape.reveal", json!({ "id": id }));
        }
    } else if resp.clicked_by(Secondary)
        && let Some(pos) = pointer
        && let Some(id) = tape_at(app, xf, pos)
    {
        run(app, "studyTape.remove", json!({ "id": id }));
    }
}

fn tape_at(app: &DrawCraftApp, xf: Affine2, pos: Pos2) -> Option<u64> {
    let page = xf.to_page(Point::new(f64::from(pos.x), f64::from(pos.y)));
    app.session
        .doc
        .study_tapes
        .iter()
        .rev()
        .find(|t| t.contains(page.x as f32, page.y as f32))
        .map(|t| t.id)
}

fn place_tape(app: &mut DrawCraftApp, xf: Affine2, a: Pos2, b: Pos2) {
    let pa = xf.to_page(Point::new(f64::from(a.x), f64::from(a.y)));
    let pb = xf.to_page(Point::new(f64::from(b.x), f64::from(b.y)));
    let page_w = f64::from(app.session.doc.width);
    let page_h = f64::from(app.session.doc.height);
    let x0 = pa.x.min(pb.x).clamp(0.0, page_w);
    let y0 = pa.y.min(pb.y).clamp(0.0, page_h);
    let x1 = pa.x.max(pb.x).clamp(0.0, page_w);
    let y1 = pa.y.max(pb.y).clamp(0.0, page_h);
    if x1 - x0 < MIN_TAPE_PAGE_SIDE || y1 - y0 < MIN_TAPE_PAGE_SIDE {
        return;
    }
    run(
        app,
        "studyTape.add",
        json!({ "x": x0, "y": y0, "width": x1 - x0, "height": y1 - y0 }),
    );
}

/// Executes an engine command and keeps its error (if any) for the status line.
fn run(app: &mut DrawCraftApp, id: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    match app.session.execute(id, &params) {
        Ok(v) => {
            app.view.study.status = None;
            Some(v)
        }
        Err(e) => {
            app.view.study.status = Some(e.to_string());
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Toolbar buttons
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// Preferences card (right settings drawer)
// ---------------------------------------------------------------------------------------------

fn toggle_row(ui: &mut egui::Ui, value: &mut bool, label: &str, hint: &str, t: &Tokens) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT);
        let width = ui.available_width();
        let resp = ui.add_sized(
            vec2(width, ROW_HEIGHT),
            egui::Checkbox::new(value, RichText::new(label).color(t.text)),
        );
        changed = resp.changed();
        resp.on_hover_text(hint);
    });
    changed
}

/// Left-handed layout, Pencil-only vs finger drawing and dark PDF pages.
pub(crate) fn draw_study_prefs_card(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let mut left = app.session.prefs.left_handed;
    let mut finger = app.session.prefs.finger_drawing;
    let mut dark_pdf = app.session.prefs.pdf_dark_mode;
    Frame::new()
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.label(RichText::new("Writing and reading").strong().color(t.text));
            ui.label(
                RichText::new("How the Pencil, your fingers and PDF pages behave.")
                    .size(12.0)
                    .color(t.text_muted),
            );
            ui.add_space(4.0);
            if toggle_row(
                ui,
                &mut finger,
                "Draw with finger as well",
                "Off: the Pencil draws and a finger pans or zooms (palm rejection). On: one finger draws too.",
                t,
            ) {
                run(
                    app,
                    "prefs.setStudy",
                    json!({ "finger_drawing": finger }),
                );
            }
            if toggle_row(
                ui,
                &mut left,
                "Left-handed layout",
                "Mirrors the pages and settings drawers so your palm never rests on them.",
                t,
            ) {
                run(app, "prefs.setStudy", json!({ "left_handed": left }));
            }
            if toggle_row(
                ui,
                &mut dark_pdf,
                "Dark PDF pages",
                "Inverts page lightness for night reading; colors and annotations stay readable.",
                t,
            ) {
                run(app, "prefs.setStudy", json!({ "pdf_dark_mode": dark_pdf }));
            }
            if app.session.prefs.finger_drawing {
                ui.label(
                    RichText::new("Pencil-only is off: a resting palm may leave marks.")
                        .size(12.0)
                        .color(t.warning_fg),
                );
            }
        });
}

// ---------------------------------------------------------------------------------------------
// Flashcards window
// ---------------------------------------------------------------------------------------------

fn now_ms() -> i64 {
    drawcraft_sync_protocol::now_unix_ms()
}

/// A card shown in the review tab: (id, front, back, source page).
type DueCard = (u64, String, String, Option<u32>);

/// Earliest due card and the number of due cards.
fn next_due(app: &DrawCraftApp, now: i64) -> (Option<DueCard>, usize) {
    let mut due = 0;
    let mut best: Option<&drawcraft_doc::Flashcard> = None;
    for card in app.session.doc.flashcards.iter().filter(|c| c.is_due(now)) {
        due += 1;
        if best.is_none_or(|b| (card.schedule.due_ms, card.id) < (b.schedule.due_ms, b.id)) {
            best = Some(card);
        }
    }
    (
        best.map(|c| (c.id, c.front.clone(), c.back.clone(), c.source_page)),
        due,
    )
}

/// Colour role of a [`flash_button`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Primary,
    Quiet,
    Again,
    Hard,
    Good,
    Easy,
}

/// Animated button: fill eases on hover, presses sink by a pixel, grades tint their label.
fn flash_button(ui: &mut egui::Ui, label: &str, tone: Tone, width: f32, t: &Tokens) -> Response {
    let height = crate::icons::min_target(ui.ctx(), ROW_HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let hover = crate::motion::hover_factor(ui.ctx(), resp.id.with("hov"), resp.hovered());
        let press = crate::motion::hover_factor(
            ui.ctx(),
            resp.id.with("prs"),
            resp.is_pointer_button_down_on(),
        );
        let (rest, lit, fg) = match tone {
            Tone::Primary => (t.accent, t.accent_hover, Color32::WHITE),
            Tone::Quiet => (t.pill_active, t.border, t.text),
            Tone::Again => (t.pill_active, t.danger_bg, t.danger_fg),
            Tone::Hard => (t.pill_active, t.ghost_hover, t.warning_fg),
            Tone::Good => (t.pill_active, t.success_bg, t.success_fg),
            Tone::Easy => (t.pill_active, t.accent_soft, t.accent),
        };
        let fill = crate::motion::lerp_color(rest, lit, hover);
        let drawn = rect.shrink(press);
        ui.painter().rect_filled(
            drawn,
            CornerRadius::same(crate::theme::CONTROL_RADIUS),
            fill,
        );
        ui.painter().text(
            drawn.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::new(
                14.0,
                egui::FontFamily::Name(crate::theme::MEDIUM_FAMILY.into()),
            ),
            fg,
        );
    }
    resp
}

fn big_button(ui: &mut egui::Ui, label: &str, t: &Tokens, primary: bool) -> Response {
    let width = ui.available_width().clamp(96.0, 160.0);
    let tone = if primary { Tone::Primary } else { Tone::Quiet };
    flash_button(ui, label, tone, width, t)
}
fn draw_flashcards_window(app: &mut DrawCraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut open = app.view.study.flashcards_open;
    // Opens with a short fade and a small rise instead of popping in.
    let enter = ctx.animate_bool_with_time(
        egui::Id::NULL.with("drawcraft_flashcards_enter"),
        open,
        crate::motion::STANDARD_DURATION_S,
    );
    let rise = (1.0 - enter) * 10.0;
    egui::Window::new("Flashcards")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, rise))
        .default_width(420.0)
        .frame(
            Frame::window(&ctx.global_style())
                .fill(t.field_bg)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(crate::theme::CARD_RADIUS))
                .inner_margin(Margin::same(16)),
        )
        .show(ctx, |ui| {
            ui.multiply_opacity(enter);
            let options = [
                crate::icons::SwitchOption::new(None, Some("Review"), "Review due cards", true),
                crate::icons::SwitchOption::new(None, Some("Create"), "Create a card", true),
            ];
            let current = match app.view.study.flashcard_tab {
                FlashcardTab::Review => 0,
                FlashcardTab::Create => 1,
            };
            let (picked, _) = crate::icons::segmented_switch(
                ui,
                "drawcraft_flashcard_tabs",
                &options,
                Some(current),
                crate::icons::SwitchLayout::EqualWidth,
                t,
            );
            if let Some(next) = picked {
                app.view.study.flashcard_tab = if next == 0 {
                    FlashcardTab::Review
                } else {
                    FlashcardTab::Create
                };
                app.view.study.answer_shown = false;
            }
            ui.add_space(12.0);
            match app.view.study.flashcard_tab {
                FlashcardTab::Review => draw_review_tab(app, ui, t),
                FlashcardTab::Create => draw_create_tab(app, ui, t),
            }
            if let Some(msg) = app.view.study.status.clone() {
                ui.add_space(6.0);
                ui.label(RichText::new(msg).size(12.0).color(t.danger_fg));
            }
        });
    app.view.study.flashcards_open = open;
}
fn draw_review_tab(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let now = now_ms();
    let total = app.session.doc.flashcards.len();
    let (next, due) = next_due(app, now);
    ui.label(
        RichText::new(format!("{due} due · {total} total"))
            .size(12.0)
            .color(t.text_muted),
    );
    // Thin progress bar: the share of cards that are already done for today, eased.
    let done = if total == 0 {
        0.0
    } else {
        (total.saturating_sub(due)) as f32 / total as f32
    };
    let eased = ui.ctx().animate_value_with_time(
        egui::Id::NULL.with("drawcraft_flashcards_progress"),
        done,
        crate::motion::STANDARD_DURATION_S,
    );
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(bar, CornerRadius::same(2), t.pill_active);
    if eased > 0.001 {
        let fill = Rect::from_min_size(bar.min, vec2(bar.width() * eased.clamp(0.0, 1.0), 4.0));
        ui.painter()
            .rect_filled(fill, CornerRadius::same(2), t.accent);
    }
    ui.add_space(10.0);
    let Some((id, front, back, source)) = next else {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(if total == 0 {
                    "No cards yet"
                } else {
                    "All caught up"
                })
                .family(egui::FontFamily::Name(crate::theme::DISPLAY_FAMILY.into()))
                .size(17.0)
                .color(t.text),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(if total == 0 {
                    "Open the Create tab to add one."
                } else {
                    "Nothing is due right now."
                })
                .size(12.5)
                .color(t.text_muted),
            );
        });
        ui.add_space(8.0);
        return;
    };

    // The card lifts slightly on hover; the answer slides open below the question.
    let card_id = ui.make_persistent_id(("drawcraft_flashcard_card", id));
    let card_hover = crate::motion::hover_factor(
        ui.ctx(),
        card_id.with("hov"),
        ui.rect_contains_pointer(ui.available_rect_before_wrap()),
    );
    let card = Frame::new()
        .fill(t.panel_bg)
        .stroke(Stroke::new(
            1.0,
            crate::motion::lerp_color(t.border, t.text_muted.gamma_multiply(0.5), card_hover),
        ))
        .corner_radius(CornerRadius::same(crate::theme::CARD_RADIUS))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_min_height(104.0);
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&front).size(18.0).color(t.text));
            let shown = app.view.study.answer_shown;
            crate::motion::animated_collapse(ui, card_id.with("answer"), shown, 56.0, |ui| {
                ui.add_space(8.0);
                let (line, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter()
                    .hline(line.x_range(), line.center().y, Stroke::new(1.0, t.border));
                ui.add_space(8.0);
                ui.label(RichText::new(&back).size(16.0).color(t.text));
            });
        });
    let _ = card;
    ui.add_space(12.0);
    if !app.view.study.answer_shown {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if big_button(ui, "Show answer", t, true).clicked() {
                app.view.study.answer_shown = true;
            }
            if let Some(page) = source
                && big_button(ui, &format!("Go to page {}", page + 1), t, false).clicked()
            {
                run(app, "page.select", json!({ "index": page }));
            }
        });
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let width = ((ui.available_width() - 24.0) / 4.0).max(60.0);
        for (label, tone, grade) in [
            ("Again", Tone::Again, ReviewGrade::Again),
            ("Hard", Tone::Hard, ReviewGrade::Hard),
            ("Good", Tone::Good, ReviewGrade::Good),
            ("Easy", Tone::Easy, ReviewGrade::Easy),
        ] {
            if flash_button(ui, label, tone, width, t).clicked() {
                run(
                    app,
                    "flashcard.review",
                    json!({ "id": id, "grade": grade, "now_ms": now }),
                );
                app.view.study.answer_shown = false;
            }
        }
    });
    ui.add_space(4.0);
    if ui
        .add_sized(
            vec2(140.0, ROW_HEIGHT),
            egui::Button::new(RichText::new("Delete card").color(t.danger_fg)).frame(false),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        run(app, "flashcard.remove", json!({ "id": id }));
        app.view.study.answer_shown = false;
    }
}
fn draw_create_tab(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.label(RichText::new("Front").size(12.0).color(t.text_muted));
    ui.add(
        egui::TextEdit::multiline(&mut app.view.study.card_front)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("Question or term"),
    );
    ui.add_space(4.0);
    ui.label(RichText::new("Back").size(12.0).color(t.text_muted));
    ui.add(
        egui::TextEdit::multiline(&mut app.view.study.card_back)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text("Answer"),
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut app.view.study.card_link_page,
        "Link to the current page",
    );
    ui.add_space(6.0);
    let ready =
        !app.view.study.card_front.trim().is_empty() && !app.view.study.card_back.trim().is_empty();
    let add = ui
        .add_enabled_ui(ready, |ui| big_button(ui, "Add card", t, true))
        .inner;
    if add.clicked() {
        let page = app
            .view
            .study
            .card_link_page
            .then(|| app.session.doc.active_page_index());
        let params = json!({
            "front": app.view.study.card_front.clone(),
            "back": app.view.study.card_back.clone(),
            "source_page": page,
        });
        if run(app, "flashcard.add", params).is_some() {
            app.view.study.card_front.clear();
            app.view.study.card_back.clear();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Command palette and notebook search
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteAction {
    Undo,
    Redo,
    AddPage,
    DuplicatePage,
    TogglePages,
    ToggleSettings,
    FitPage,
    FitWidth,
    StudyTape,
    Flashcards,
    ToggleTheme,
    ToggleLeftHanded,
    ToggleFingerDrawing,
    TogglePdfDark,
    FindInNotebook,
}

const PALETTE_ACTIONS: &[(PaletteAction, &str, &str)] = &[
    (PaletteAction::Undo, "Undo", "Cmd/Ctrl+Z"),
    (PaletteAction::Redo, "Redo", "Cmd/Ctrl+Shift+Z"),
    (PaletteAction::AddPage, "Add page", ""),
    (PaletteAction::DuplicatePage, "Duplicate page", ""),
    (PaletteAction::TogglePages, "Show or hide pages", "P"),
    (PaletteAction::ToggleSettings, "Show or hide settings", "S"),
    (PaletteAction::FitWidth, "Fit page width", "Cmd/Ctrl+0"),
    (PaletteAction::FitPage, "Fit whole page", "Cmd/Ctrl+9"),
    (PaletteAction::StudyTape, "Study tape tool", ""),
    (PaletteAction::Flashcards, "Flashcards", ""),
    (
        PaletteAction::FindInNotebook,
        "Find in notebook",
        "Cmd/Ctrl+F",
    ),
    (PaletteAction::ToggleTheme, "Switch light or dark theme", ""),
    (
        PaletteAction::ToggleLeftHanded,
        "Toggle left-handed layout",
        "",
    ),
    (
        PaletteAction::ToggleFingerDrawing,
        "Toggle drawing with finger",
        "",
    ),
    (PaletteAction::TogglePdfDark, "Toggle dark PDF pages", ""),
];

fn matches_query(label: &str, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || q.split_whitespace()
            .all(|w| label.to_lowercase().contains(w))
}

/// `(page index, snippet)` for text boxes containing `query` (case-insensitive).
fn find_in_notebook(app: &DrawCraftApp, query: &str) -> Vec<(usize, String)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let doc = &app.session.doc;
    let active = doc.active_page_index();
    let mut out = Vec::new();
    for (idx, page) in doc.pages.iter().enumerate() {
        // The active page's live content lives in the document's top-level fields.
        let texts = if idx == active {
            &doc.texts
        } else {
            &page.texts
        };
        for item in texts {
            let lower = item.text.to_lowercase();
            if let Some(at) = lower.find(&q) {
                let start = item
                    .text
                    .char_indices()
                    .map(|(i, _)| i)
                    .rfind(|i| *i <= at.saturating_sub(20))
                    .unwrap_or(0);
                let snippet: String = item.text[start..].chars().take(60).collect();
                out.push((idx, snippet.replace('\n', " ")));
                if out.len() >= MAX_FIND_RESULTS {
                    return out;
                }
                break;
            }
        }
    }
    out
}

fn run_palette_action(app: &mut DrawCraftApp, action: PaletteAction) {
    match action {
        PaletteAction::Undo => {
            run(app, "edit.undo", json!({}));
        }
        PaletteAction::Redo => {
            run(app, "edit.redo", json!({}));
        }
        PaletteAction::AddPage => {
            run(app, "page.add", json!({}));
        }
        PaletteAction::DuplicatePage => {
            run(app, "page.duplicate", json!({}));
        }
        PaletteAction::TogglePages => app.view.pages_sidebar_open = !app.view.pages_sidebar_open,
        PaletteAction::ToggleSettings => app.view.inspector_open = !app.view.inspector_open,
        PaletteAction::FitPage => app.view.fit_requested = true,
        PaletteAction::FitWidth => fit_width(app),
        PaletteAction::StudyTape => toggle_tape_mode(app),
        PaletteAction::Flashcards => app.view.study.flashcards_open = true,
        PaletteAction::FindInNotebook => open_palette(app, true),
        PaletteAction::ToggleTheme => {
            let next = match app.session.prefs.theme {
                drawcraft_doc::UiTheme::Dark => drawcraft_doc::UiTheme::Light,
                _ => drawcraft_doc::UiTheme::Dark,
            };
            run(app, "theme.set", json!({ "theme": next }));
        }
        PaletteAction::ToggleLeftHanded => {
            let v = !app.session.prefs.left_handed;
            run(app, "prefs.setStudy", json!({ "left_handed": v }));
        }
        PaletteAction::ToggleFingerDrawing => {
            let v = !app.session.prefs.finger_drawing;
            run(app, "prefs.setStudy", json!({ "finger_drawing": v }));
        }
        PaletteAction::TogglePdfDark => {
            let v = !app.session.prefs.pdf_dark_mode;
            run(app, "prefs.setStudy", json!({ "pdf_dark_mode": v }));
        }
    }
}

enum PaletteRow {
    Action(PaletteAction, &'static str, &'static str),
    Page(usize, String),
}

fn palette_rows(app: &DrawCraftApp) -> Vec<PaletteRow> {
    let s = &app.view.study;
    if s.palette_find {
        return find_in_notebook(app, &s.palette_query)
            .into_iter()
            .map(|(page, snippet)| PaletteRow::Page(page, snippet))
            .collect();
    }
    PALETTE_ACTIONS
        .iter()
        .filter(|(_, label, _)| matches_query(label, &s.palette_query))
        .take(MAX_PALETTE_ROWS)
        .map(|(a, label, hint)| PaletteRow::Action(*a, label, hint))
        .collect()
}

fn draw_palette(app: &mut DrawCraftApp, ctx: &egui::Context, t: &Tokens) {
    let rows = palette_rows(app);
    let (up, down, enter, escape) = ctx.input(|i| {
        (
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Escape),
        )
    });
    let s = &mut app.view.study;
    if down && !rows.is_empty() {
        s.palette_selected = (s.palette_selected + 1) % rows.len();
    }
    if up && !rows.is_empty() {
        s.palette_selected = (s.palette_selected + rows.len() - 1) % rows.len();
    }
    s.palette_selected = s.palette_selected.min(rows.len().saturating_sub(1));
    let mut chosen: Option<usize> = enter.then_some(s.palette_selected);
    let mut close = escape;
    let find = s.palette_find;
    let mut query = std::mem::take(&mut s.palette_query);
    let selected = s.palette_selected;
    let want_focus = std::mem::take(&mut s.palette_focus);

    egui::Area::new(egui::Id::unique("drawcraft_command_palette"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 72.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style())
                .fill(t.panel_bg)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(440.0_f32.min(ctx.content_rect().width() - 32.0));
                    let edit = ui.add_sized(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        egui::TextEdit::singleline(&mut query).hint_text(if find {
                            "Find text in this notebook"
                        } else {
                            "Type a command"
                        }),
                    );
                    if want_focus {
                        edit.request_focus();
                    }
                    ui.add_space(4.0);
                    if rows.is_empty() {
                        ui.label(
                            RichText::new(if find && query.trim().is_empty() {
                                "Start typing to search every page."
                            } else {
                                "Nothing matches."
                            })
                            .color(t.text_muted),
                        );
                    }
                    for (idx, row) in rows.iter().enumerate() {
                        let (label, hint) = match row {
                            PaletteRow::Action(_, l, h) => ((*l).to_string(), (*h).to_string()),
                            PaletteRow::Page(p, snip) => (snip.clone(), format!("Page {}", p + 1)),
                        };
                        let (rect, resp) = ui.allocate_exact_size(
                            vec2(ui.available_width(), ROW_HEIGHT),
                            egui::Sense::click(),
                        );
                        if idx == selected || resp.hovered() {
                            ui.painter()
                                .rect_filled(rect, CornerRadius::same(8), t.accent_soft);
                        }
                        ui.painter().text(
                            rect.left_center() + vec2(10.0, 0.0),
                            Align2::LEFT_CENTER,
                            label,
                            FontId::proportional(14.0),
                            t.text,
                        );
                        ui.painter().text(
                            rect.right_center() - vec2(10.0, 0.0),
                            Align2::RIGHT_CENTER,
                            hint,
                            FontId::proportional(12.0),
                            t.text_muted,
                        );
                        if resp.clicked() {
                            chosen = Some(idx);
                        }
                    }
                });
        });

    app.view.study.palette_query = query;
    if let Some(idx) = chosen
        && let Some(row) = rows.get(idx)
    {
        close = true;
        match row {
            PaletteRow::Action(action, _, _) => {
                let action = *action;
                // Leave the palette first so actions that reopen it (Find) take effect.
                app.view.study.palette_open = false;
                run_palette_action(app, action);
                close = !app.view.study.palette_open;
            }
            PaletteRow::Page(page, _) => {
                run(app, "page.select", json!({ "index": page }));
            }
        }
    }
    if close {
        app.view.study.palette_open = false;
    }
}

/// Records which touches are the Pencil and which are fingers (reported by the platform).
/// Seeing a Pencil touch also marks the Pencil as present, which enables Pencil-only mode.
///
/// Entries are kept in a bounded 64-entry ring rather than deleted on `active == false` so a
/// quick tap whose `Start` and `End` arrive within the same frame still resolves its true source
/// when `canvas.rs` processes the frame's events.
pub(crate) fn poll_touch_sources(app: &mut DrawCraftApp) {
    if let Some(poll_direct) = app.hooks.pointer_is_direct_touch.as_mut() {
        app.view.study.pointer_is_direct_touch = poll_direct();
    }
    let Some(poll) = app.hooks.touch_sources.as_mut() else {
        return;
    };
    for ev in poll() {
        let list = &mut app.view.study.touch_sources;
        list.retain(|(id, _)| *id != ev.id);
        if list.len() >= 64 {
            list.remove(0);
        }
        list.push((ev.id, ev.source));
        if ev.source == crate::TouchSource::Stylus {
            app.stylus.hardware_seen_in_session = true;
            app.view.study.pointer_is_direct_touch = false;
        }
    }
}

impl StudyUiState {
    pub(crate) fn note_fonts_installed(&mut self, frame: u64) {
        self.brand_fonts_frame = Some(frame);
    }

    /// Applies the Inter / Geist text scale once egui has bound the new font families.
    pub(crate) fn tick_brand_styles(&mut self, ctx: &egui::Context, frame: u64) {
        if self.brand_styles_applied {
            return;
        }
        if self
            .brand_fonts_frame
            .is_some_and(|installed| frame > installed)
        {
            crate::theme::apply_brand_text_styles(ctx);
            self.brand_styles_applied = true;
        }
    }

    /// Declares the source of a live touch (used by the platform layer and tests).
    pub fn set_touch_source(&mut self, id: i32, source: crate::TouchSource) {
        self.touch_sources.retain(|(i, _)| *i != id);
        self.touch_sources.push((id, source));
    }

    /// Source of a live touch, if the platform reported one.
    #[must_use]
    pub fn touch_source(&self, id: egui::TouchId) -> Option<crate::TouchSource> {
        self.touch_sources
            .iter()
            .find(|(i, _)| egui::TouchId::from(*i) == id)
            .map(|(_, s)| *s)
    }
}

/// Applies two-finger (undo) and three-finger (redo) taps reported by the platform layer.
fn poll_gestures(app: &mut DrawCraftApp) {
    let Some(poll) = app.hooks.tablet_gestures.as_mut() else {
        return;
    };
    let gestures = poll();
    if app.view.screen != crate::AppScreen::Editor || app.view.active_modal.is_some() {
        return;
    }
    for gesture in gestures {
        app.view.selection_drag = None;
        let command = match gesture {
            crate::TabletGesture::Undo => "edit.undo",
            crate::TabletGesture::Redo => "edit.redo",
        };
        run(app, command, json!({}));
    }
}

fn storage_indicator(app: &mut DrawCraftApp) -> Option<crate::StorageIndicator> {
    app.hooks.storage_status.as_mut().map(|status| status())
}

/// Small "Saved / Saving / Not saved" label shown in the toolbar. "Saved" appears only after the
/// browser confirmed a durable write.
pub(crate) fn draw_save_status(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some(indicator) = storage_indicator(app) else {
        return;
    };
    let (text, color) = match indicator {
        crate::StorageIndicator::Saved => ("Saved", t.text_muted),
        crate::StorageIndicator::Saving => ("Saving…", t.text_muted),
        crate::StorageIndicator::Unsaved => ("Not saved yet", t.warning_fg),
        crate::StorageIndicator::Full => ("Storage full", t.danger_fg),
        crate::StorageIndicator::Failed(_) => ("Not saved", t.danger_fg),
    };
    ui.label(RichText::new(text).size(12.0).color(color));
}

/// Banner with export fallbacks when the browser refuses to store the workspace.
fn draw_storage_banner(app: &mut DrawCraftApp, ctx: &egui::Context, t: &Tokens) {
    let Some(indicator) = storage_indicator(app) else {
        return;
    };
    let message = match indicator {
        crate::StorageIndicator::Full => {
            "Browser storage is full, so your latest changes are not saved. Export your work now."
                .to_string()
        }
        crate::StorageIndicator::Failed(why) => {
            format!("Your latest changes could not be saved ({why}). Export your work to keep it.")
        }
        _ => return,
    };
    let mut export_notebook = false;
    let mut export_all = false;
    egui::Area::new(egui::Id::unique("drawcraft_storage_banner"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -16.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style())
                .fill(t.danger_bg)
                .stroke(Stroke::new(1.0, t.danger_border))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(460.0_f32.min(ctx.content_rect().width() - 32.0));
                    ui.label(RichText::new(message).color(t.danger_fg));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        export_notebook = big_button(ui, "Export notebook", t, true).clicked();
                        export_all = big_button(ui, "Export everything", t, false).clicked();
                    });
                });
        });
    if export_notebook {
        crate::modal::open(app, crate::modal::ActiveModal::Export);
    }
    if export_all {
        let _ = app.trigger_export_package_dialog();
    }
}

/// Small floating control that leaves focus mode (all chrome is hidden there).
fn draw_exit_focus(app: &mut DrawCraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut leave = false;
    egui::Area::new(egui::Id::unique("drawcraft_exit_focus"))
        .order(egui::Order::Foreground)
        .anchor(Align2::RIGHT_TOP, vec2(-12.0, 12.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style())
                .fill(t.panel_bg)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(crate::theme::CARD_RADIUS))
                .inner_margin(Margin::same(4))
                .show(ui, |ui| {
                    leave = compact_icon_button(ui, Icon::FitPage, None, true, true, t)
                        .on_hover_text("Leave focus mode")
                        .clicked();
                });
        });
    if leave || ctx.input(|i| i.key_pressed(Key::Escape)) {
        app.view.study.focus_mode = false;
    }
}

/// Draws the flashcard window and command palette on top of the editor.
pub(crate) fn show_overlays(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let t = Tokens::for_theme(app.session.prefs.theme);
    crate::pdf_view::set_pdf_dark(ctx, app.session.prefs.pdf_dark_mode);
    crate::icons::set_touch_ui(ctx, app.view.study.touch_ui);
    if ctx.input(|i| {
        i.events
            .iter()
            .any(|e| matches!(e, egui::Event::Touch { .. }))
    }) && !app.view.study.touch_ui
    {
        app.view.study.touch_ui = true;
    }
    poll_gestures(app);
    draw_storage_banner(app, ctx, &t);
    if app.view.screen != crate::AppScreen::Editor {
        return;
    }
    if app.view.study.focus_mode {
        draw_exit_focus(app, ctx, &t);
    }
    if app.view.study.flashcards_open {
        draw_flashcards_window(app, ctx, &t);
    }
    if app.view.study.palette_open {
        draw_palette(app, ctx, &t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_query_matches_every_word_case_insensitively() {
        assert!(matches_query("Toggle dark PDF pages", "pdf DARK"));
        assert!(matches_query("Undo", ""));
        assert!(!matches_query("Undo", "redo"));
    }

    #[test]
    fn every_palette_action_has_a_unique_label() {
        let mut labels: Vec<&str> = PALETTE_ACTIONS.iter().map(|(_, l, _)| *l).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(before, labels.len());
    }

    #[test]
    fn tape_rect_scales_with_the_view() {
        let tape = StudyTapeItem::new(1, 10.0, 20.0, 100.0, 40.0);
        let r = tape_screen_rect(&tape, Affine2::new(2.0, 5.0, 7.0));
        assert!((r.min.x - 25.0).abs() < 1e-3 && (r.min.y - 47.0).abs() < 1e-3);
        assert!((r.width() - 200.0).abs() < 1e-3 && (r.height() - 80.0).abs() < 1e-3);
    }

    fn app_with_canvas() -> DrawCraftApp {
        let mut app = DrawCraftApp::default();
        app.view.last_canvas_rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0)));
        app
    }

    #[test]
    fn dragging_places_a_tape_through_the_engine_and_tapping_reveals_it() {
        let mut app = app_with_canvas();
        let xf = Affine2::new(1.0, 0.0, 0.0);
        place_tape(&mut app, xf, pos2(100.0, 100.0), pos2(300.0, 160.0));
        assert_eq!(app.session.doc.study_tapes.len(), 1);
        let tape = app.session.doc.study_tapes[0];
        assert!(!tape.revealed);
        assert_eq!(tape_at(&app, xf, pos2(150.0, 120.0)), Some(tape.id));
        assert_eq!(tape_at(&app, xf, pos2(10.0, 10.0)), None);

        run(&mut app, "studyTape.reveal", json!({ "id": tape.id }));
        assert!(app.session.doc.study_tapes[0].revealed);
        run(&mut app, "edit.undo", json!({}));
        assert!(
            app.session.doc.study_tapes.is_empty(),
            "placing a tape is one undo step"
        );
    }

    #[test]
    fn tiny_or_off_page_drags_do_not_create_tapes() {
        let mut app = app_with_canvas();
        let xf = Affine2::new(1.0, 0.0, 0.0);
        place_tape(&mut app, xf, pos2(100.0, 100.0), pos2(104.0, 140.0));
        place_tape(&mut app, xf, pos2(-500.0, -500.0), pos2(-300.0, -300.0));
        assert!(app.session.doc.study_tapes.is_empty());
    }

    #[test]
    fn tape_mode_is_exclusive_with_other_overlay_tools() {
        let mut app = app_with_canvas();
        app.view.shape_insert_mode = true;
        toggle_tape_mode(&mut app);
        assert!(app.view.study.tape_mode && !app.view.shape_insert_mode);
        leave_tape_mode(&mut app);
        assert!(!app.view.study.tape_mode);
        toggle_tape_mode(&mut app);
        toggle_tape_mode(&mut app);
        assert!(
            !app.view.study.tape_mode,
            "second toggle turns the tool off"
        );
    }

    #[test]
    fn fit_width_fills_the_canvas_width_and_pins_the_page_top() {
        let mut app = app_with_canvas();
        fit_width(&mut app);
        let page_w = f64::from(app.session.doc.width);
        let drawn = page_w * f64::from(app.view.zoom);
        assert!(
            (drawn - (1000.0 - 48.0)).abs() < 2.0,
            "page spans canvas minus padding: {drawn}"
        );
        assert!((app.view.pan[1] - 24.0).abs() < 1e-6);
        assert!((app.session.zoom - app.view.zoom).abs() < 1e-6);
    }

    #[test]
    fn notebook_search_covers_every_page_including_the_active_one() {
        let mut app = app_with_canvas();
        run(
            &mut app,
            "text.insert",
            json!({ "text": "Krebs cycle summary", "x": 40.0, "y": 40.0 }),
        );
        run(&mut app, "page.add", json!({}));
        run(
            &mut app,
            "text.insert",
            json!({ "text": "Photosynthesis and the Krebs cycle", "x": 40.0, "y": 40.0 }),
        );
        let hits = find_in_notebook(&app, "KREBS");
        let pages: Vec<usize> = hits.iter().map(|(p, _)| *p).collect();
        assert_eq!(pages, vec![0, 1]);
        assert!(find_in_notebook(&app, "   ").is_empty());
        assert!(find_in_notebook(&app, "zzz-not-there").is_empty());
    }

    #[test]
    fn palette_actions_run_engine_commands() {
        let mut app = app_with_canvas();
        run_palette_action(&mut app, PaletteAction::ToggleFingerDrawing);
        run_palette_action(&mut app, PaletteAction::ToggleLeftHanded);
        run_palette_action(&mut app, PaletteAction::TogglePdfDark);
        assert!(app.session.prefs.finger_drawing);
        assert!(app.session.prefs.left_handed);
        assert!(app.session.prefs.pdf_dark_mode);
        let pages = app.session.doc.page_count();
        run_palette_action(&mut app, PaletteAction::AddPage);
        assert_eq!(app.session.doc.page_count(), pages + 1);
        run_palette_action(&mut app, PaletteAction::Undo);
        assert_eq!(app.session.doc.page_count(), pages);
    }

    #[test]
    fn same_frame_touch_start_and_end_preserves_touch_source() {
        let mut app = app_with_canvas();
        app.hooks.touch_sources = Some(Box::new(|| {
            vec![
                crate::TouchSourceEvent {
                    id: 77,
                    source: crate::TouchSource::Direct,
                    active: true,
                },
                crate::TouchSourceEvent {
                    id: 77,
                    source: crate::TouchSource::Direct,
                    active: false,
                },
                crate::TouchSourceEvent {
                    id: 78,
                    source: crate::TouchSource::Stylus,
                    active: true,
                },
                crate::TouchSourceEvent {
                    id: 78,
                    source: crate::TouchSource::Stylus,
                    active: false,
                },
            ]
        }));
        poll_touch_sources(&mut app);
        assert_eq!(
            app.view.study.touch_source(egui::TouchId::from(77)),
            Some(crate::TouchSource::Direct),
            "quick same-frame finger tap must keep its Direct source for canvas.rs"
        );
        assert_eq!(
            app.view.study.touch_source(egui::TouchId::from(78)),
            Some(crate::TouchSource::Stylus),
            "quick same-frame Pencil tap must keep its Stylus source for canvas.rs"
        );
        assert!(app.stylus.hardware_seen_in_session);
    }
}
