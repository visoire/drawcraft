//! Focused modal bodies for creating notebooks and editing notebook metadata.

use std::sync::Arc;

use drawcraft_color::Color;
use drawcraft_doc::{
    NotebookCover, PageOrientation, PageResolutionPreset, PaperColor, PaperTemplate,
    ResolvedPaperColors, UiTheme,
};
use egui::{Color32, CornerRadius, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2};
use serde_json::json;

use crate::DrawCraftApp;
use crate::color_picker::{paint_hue_bar_picker, paint_sv_square_picker};
use crate::icons::{
    Icon, SwitchLayout, SwitchOption, chip_close_button, compact_icon_button, icon_button,
    paint_icon, segmented_switch,
};
use crate::library_folders::{
    FolderPickerDrafts, commit_pending_picker_folder, draw_folder_picker, known_library_folders,
    library_tags_with_counts,
};
use crate::modal::{self, danger_action_button, primary_action_button, secondary_action_button};
use crate::page_format_picker::{
    PageFormatDraft, PageFormatPreviewContext, render_page_format_picker,
};
use crate::theme::{Tokens, to_color32};

const CUSTOM_COVER_QUICK_HEXES: &[(&str, &str)] = &[
    ("Cobalt Blue", "#1D4ED8"),
    ("Deep Teal", "#0F766E"),
    ("Forest Pine", "#15803D"),
    ("Warm Amber", "#B45309"),
    ("Crimson Berry", "#BE123C"),
    ("Royal Violet", "#7C3AED"),
    ("Dusty Rose", "#DB2777"),
    ("Midnight Ink", "#111827"),
];

/// Interactive draft state for choosing a custom RGB notebook cover color.
#[derive(Clone, Debug)]
pub struct CoverColorDraftState {
    pub open: bool,
    /// `[hue (0..=1), saturation (0..=1), value (0..=1)]`.
    pub hsv: [f32; 3],
    pub hex_draft: String,
}

impl Default for CoverColorDraftState {
    fn default() -> Self {
        Self::from_cover(NotebookCover::ClassicNavy)
    }
}

impl CoverColorDraftState {
    /// Initializes a draft from `cover`, opening the inline color studio for custom covers.
    pub fn from_cover(cover: NotebookCover) -> Self {
        let primary = cover.primary_color().with_alpha(1.0);
        Self {
            open: cover.is_custom(),
            hsv: primary.to_hsv(),
            hex_draft: primary.to_hex(),
        }
    }

    /// Synchronizes draft state from `cover`, expanding the studio only when `cover` is custom.
    pub fn sync_from_cover(&mut self, cover: NotebookCover) {
        self.set_color(cover.primary_color());
        self.open = cover.is_custom();
    }

    pub fn set_color(&mut self, color: Color) {
        let opaque = color.with_alpha(1.0);
        self.hsv = opaque.to_hsv();
        self.hex_draft = opaque.to_hex();
    }

    pub fn current_color(&self) -> Color {
        Color::from_hsv(self.hsv[0], self.hsv[1], self.hsv[2], 1.0)
    }

    pub fn current_cover(&self) -> NotebookCover {
        NotebookCover::from_color(self.current_color())
    }
}

/// Paints a realistic hardcover book cover with spine, elastic band, shadow, and label plate.
pub(crate) fn paint_hardcover_book_cover(
    p: &egui::Painter,
    rect: Rect,
    cover: NotebookCover,
    title: Option<&str>,
    border: Color32,
) {
    let r_spine = (rect.width() * 0.045).clamp(2.0, 4.0).round() as u8;
    let r_outer = (rect.width() * 0.085).clamp(3.0, 6.0).round() as u8;
    let cover_radius = CornerRadius {
        nw: r_spine,
        sw: r_spine,
        ne: r_outer,
        se: r_outer,
    };
    let spine_radius = CornerRadius {
        nw: r_spine,
        sw: r_spine,
        ne: 0,
        se: 0,
    };

    let shadow_dy = (rect.height() * 0.028).clamp(1.5, 3.0);
    p.rect_filled(
        rect.translate(Vec2::new(0.0, shadow_dy)),
        cover_radius,
        Color32::from_black_alpha(30),
    );

    let primary = to_color32(cover.primary_color());
    let spine = to_color32(cover.spine_color());
    p.rect_filled(rect, cover_radius, primary);

    let spine_w = (rect.width() * 0.21).clamp(5.0, 16.0);
    let spine_rect = Rect::from_min_size(rect.min, Vec2::new(spine_w, rect.height()));
    p.rect_filled(spine_rect, spine_radius, spine);
    p.line_segment(
        [
            pos2(spine_rect.right(), rect.top() + 2.0),
            pos2(spine_rect.right(), rect.bottom() - 2.0),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(45)),
    );

    let has_band = rect.width() >= 24.0;
    let band_x = if has_band {
        let offset = (rect.width() * 0.11).clamp(4.0, 8.5);
        let band_w = (rect.width() * 0.042).clamp(2.0, 3.2);
        let x = rect.right() - offset;
        p.rect_filled(
            Rect::from_min_size(
                pos2(x, rect.top() + 1.0),
                Vec2::new(band_w, rect.height() - 2.0),
            ),
            0.0,
            spine.gamma_multiply(0.85),
        );
        x
    } else {
        rect.right() - 3.0
    };

    if rect.width() >= 22.0 {
        paint_cover_label_plate(p, rect, spine_rect.right(), band_x, primary, title);
    }

    p.rect_stroke(
        rect,
        cover_radius,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
}

fn paint_cover_label_plate(
    p: &egui::Painter,
    rect: Rect,
    spine_right: f32,
    band_left: f32,
    primary: Color32,
    title: Option<&str>,
) {
    let plate_x = spine_right + (rect.width() * 0.10).clamp(3.0, 8.0);
    let right_pad = (rect.width() * 0.075).clamp(2.0, 6.0);
    let plate_w = (band_left - right_pad - plate_x).max(6.0);
    let plate_h = (rect.height() * 0.25).max(5.0);
    let plate_rect = Rect::from_min_size(
        pos2(plate_x, rect.top() + rect.height() * 0.21),
        Vec2::new(plate_w, plate_h),
    );

    p.rect_filled(
        plate_rect,
        2.5,
        Color32::from_rgba_unmultiplied(252, 250, 246, 238),
    );
    p.rect_stroke(
        plate_rect,
        2.5,
        Stroke::new(0.75, Color32::from_black_alpha(40)),
        StrokeKind::Inside,
    );

    if let Some(title_txt) = title.map(str::trim).filter(|s| !s.is_empty())
        && plate_w >= 34.0
        && plate_h >= 18.0
    {
        let text_rect = plate_rect.shrink2(Vec2::new(3.0, 2.0));
        let font_size = (plate_h * 0.30).clamp(6.8, 8.5);
        let mut job = egui::text::LayoutJob::single_section(
            title_txt.to_string(),
            egui::TextFormat {
                font_id: egui::FontId::proportional(font_size),
                color: Color32::from_rgb(0x1E, 0x22, 0x2B),
                ..Default::default()
            },
        );
        job.wrap.max_width = text_rect.width().max(8.0);
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = p.ctx().fonts_mut(|f| f.layout_job(job));
        let text_y = (text_rect.center().y - galley.size().y * 0.5).max(text_rect.top());
        p.with_clip_rect(plate_rect)
            .galley(pos2(text_rect.left(), text_y), galley, Color32::BLACK);
        return;
    }

    if plate_w >= 12.0 {
        let line_y1 = plate_rect.top() + plate_rect.height() * 0.38;
        let line_y2 = plate_rect.top() + plate_rect.height() * 0.68;
        p.line_segment(
            [
                pos2(plate_rect.left() + 3.0, line_y1),
                pos2(plate_rect.right() - 3.0, line_y1),
            ],
            Stroke::new(1.3, primary.gamma_multiply(0.85)),
        );
        if plate_w >= 18.0 {
            p.line_segment(
                [
                    pos2(plate_rect.left() + 3.0, line_y2),
                    pos2(
                        (plate_rect.right() - 6.0).max(plate_rect.left() + 5.0),
                        line_y2,
                    ),
                ],
                Stroke::new(1.0, Color32::from_black_alpha(105)),
            );
        }
    }
}

fn color32_luma(c: Color32) -> f32 {
    0.299 * (f32::from(c.r()) / 255.0)
        + 0.587 * (f32::from(c.g()) / 255.0)
        + 0.114 * (f32::from(c.b()) / 255.0)
}

fn ensure_preview_line_contrast(bg: Color32, line: Color32) -> Color32 {
    let bg_luma = color32_luma(bg);
    let line_luma = color32_luma(line);
    if (bg_luma - line_luma).abs() >= 0.16 {
        return line;
    }
    if bg_luma < 0.45 {
        crate::motion::lerp_color(line, Color32::from_rgb(0xA6, 0xB2, 0xC6), 0.32)
    } else {
        crate::motion::lerp_color(line, Color32::from_rgb(0x74, 0x82, 0x98), 0.24)
    }
}

#[derive(Clone)]
struct PreviewPainter {
    p: egui::Painter,
    center: egui::Pos2,
    turns: i32,
}

impl PreviewPainter {
    fn new(p: &egui::Painter, center: egui::Pos2, turns: i32) -> Self {
        Self {
            p: p.clone(),
            center,
            turns: turns.rem_euclid(4),
        }
    }

    fn rot_pt(&self, pt: egui::Pos2) -> egui::Pos2 {
        let d = pt - self.center;
        let r = match self.turns {
            1 => Vec2::new(-d.y, d.x),
            2 => Vec2::new(-d.x, -d.y),
            3 => Vec2::new(d.y, -d.x),
            _ => d,
        };
        self.center + r
    }

    fn rot_rect(&self, rect: Rect) -> Rect {
        Rect::from_two_pos(self.rot_pt(rect.min), self.rot_pt(rect.max))
    }

    fn rect_filled(&self, rect: Rect, corner_radius: impl Into<egui::CornerRadius>, fill: Color32) {
        self.p.rect_filled(self.rot_rect(rect), corner_radius, fill);
    }

    fn rect_stroke(
        &self,
        rect: Rect,
        corner_radius: impl Into<egui::CornerRadius>,
        stroke: Stroke,
        kind: StrokeKind,
    ) {
        self.p
            .rect_stroke(self.rot_rect(rect), corner_radius, stroke, kind);
    }

    fn line_segment(&self, points: [egui::Pos2; 2], stroke: Stroke) {
        self.p
            .line_segment([self.rot_pt(points[0]), self.rot_pt(points[1])], stroke);
    }

    fn circle_filled(&self, center: egui::Pos2, radius: f32, fill: Color32) {
        self.p.circle_filled(self.rot_pt(center), radius, fill);
    }

    fn with_clip_rect(&self, rect: Rect) -> Self {
        Self {
            p: self.p.with_clip_rect(self.rot_rect(rect)),
            center: self.center,
            turns: self.turns,
        }
    }
}

/// Paints a miniature paper sheet with background tint and scaled ruling lines.
pub(crate) fn paint_paper_preview_colors(
    p: &egui::Painter,
    rect: Rect,
    template: PaperTemplate,
    colors: ResolvedPaperColors,
    border: Color32,
) {
    paint_paper_preview_colors_rotated(p, rect, template, colors, border, 0);
}

/// Paints a miniature paper sheet rotated clockwise by `quarter_turns` 90° steps.
pub(crate) fn paint_paper_preview_colors_rotated(
    p: &egui::Painter,
    rect: Rect,
    template: PaperTemplate,
    colors: ResolvedPaperColors,
    border: Color32,
    quarter_turns: i32,
) {
    let turns = quarter_turns.rem_euclid(4);
    let unrot_size = if turns % 2 == 1 {
        Vec2::new(rect.height(), rect.width())
    } else {
        rect.size()
    };
    let unrot_rect = Rect::from_center_size(rect.center(), unrot_size);
    let rp = PreviewPainter::new(p, rect.center(), turns);

    let bg = to_color32(colors.background);
    let line = ensure_preview_line_contrast(bg, to_color32(colors.line));
    let margin = to_color32(colors.margin);

    rp.rect_filled(unrot_rect, 4.0, bg);

    let left = unrot_rect.left() + 4.0;
    let right = unrot_rect.right() - 4.0;
    let top = unrot_rect.top() + 5.0;
    let bottom = unrot_rect.bottom() - 5.0;

    match template {
        PaperTemplate::Blank => {
            let fold = (unrot_rect.width() * 0.17).clamp(5.0, 9.0);
            let corner = pos2(unrot_rect.right() - fold, unrot_rect.top() + fold);
            rp.line_segment(
                [
                    pos2(unrot_rect.right() - fold, unrot_rect.top() + 1.0),
                    corner,
                ],
                Stroke::new(0.85, line),
            );
            rp.line_segment(
                [
                    corner,
                    pos2(unrot_rect.right() - 1.0, unrot_rect.top() + fold),
                ],
                Stroke::new(0.85, line),
            );
        }
        PaperTemplate::RuledNarrow => {
            paint_horizontal_rules(&rp, left, right, top + 5.0, bottom, 4.8, line);
            let mx = unrot_rect.left() + unrot_rect.width() * 0.20;
            rp.line_segment(
                [
                    pos2(mx, unrot_rect.top() + 2.0),
                    pos2(mx, unrot_rect.bottom() - 2.0),
                ],
                Stroke::new(0.95, margin),
            );
        }
        PaperTemplate::RuledWide => {
            paint_horizontal_rules(&rp, left, right, top + 7.5, bottom, 8.2, line);
            let mx = unrot_rect.left() + unrot_rect.width() * 0.20;
            rp.line_segment(
                [
                    pos2(mx, unrot_rect.top() + 2.0),
                    pos2(mx, unrot_rect.bottom() - 2.0),
                ],
                Stroke::new(0.95, margin),
            );
        }
        PaperTemplate::Grid => {
            paint_horizontal_rules(&rp, left, right, top + 4.0, bottom, 6.4, line);
            let mut x = left + 4.0;
            while x < right - 1.0 {
                rp.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(0.75, line));
                x += 6.4;
            }
        }
        PaperTemplate::Dotted => {
            let mut y = top + 4.0;
            while y < bottom - 1.0 {
                let mut x = left + 3.0;
                while x < right - 1.0 {
                    rp.circle_filled(pos2(x, y), 1.05, line);
                    x += 6.4;
                }
                y += 6.4;
            }
        }
        PaperTemplate::Cornell => {
            let header_y = unrot_rect.top() + unrot_rect.height() * 0.19;
            let footer_y = unrot_rect.bottom() - unrot_rect.height() * 0.21;
            let cue_x = unrot_rect.left() + unrot_rect.width() * 0.30;
            rp.line_segment(
                [pos2(left, header_y), pos2(right, header_y)],
                Stroke::new(0.95, margin),
            );
            rp.line_segment(
                [pos2(left, footer_y), pos2(right, footer_y)],
                Stroke::new(0.95, margin),
            );
            rp.line_segment(
                [pos2(cue_x, header_y), pos2(cue_x, footer_y)],
                Stroke::new(0.95, margin),
            );
            paint_horizontal_rules(
                &rp,
                cue_x + 3.0,
                right,
                header_y + 4.5,
                footer_y - 2.0,
                5.0,
                line,
            );
        }
        PaperTemplate::Isometric => {
            let p_clip = rp.with_clip_rect(unrot_rect.shrink(2.0));
            let step_x = 9.0_f32;
            let mut x = left + 4.0;
            while x < right - 1.0 {
                p_clip.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(0.65, line));
                x += step_x;
            }
            let dy = (right - left) * 0.577;
            let mut y = top - dy;
            while y <= bottom + dy {
                p_clip.line_segment(
                    [pos2(left, y), pos2(right, y + dy)],
                    Stroke::new(0.65, line),
                );
                p_clip.line_segment(
                    [pos2(left, y), pos2(right, y - dy)],
                    Stroke::new(0.65, line),
                );
                y += 10.4;
            }
        }
        PaperTemplate::MusicStaff => {
            let mut staff_y = top + 3.0;
            while staff_y + 10.0 <= bottom {
                for l in 0..5 {
                    let ly = staff_y + l as f32 * 2.2;
                    rp.line_segment([pos2(left, ly), pos2(right, ly)], Stroke::new(0.65, line));
                }
                rp.line_segment(
                    [pos2(left, staff_y), pos2(left, staff_y + 8.8)],
                    Stroke::new(0.85, margin),
                );
                rp.line_segment(
                    [pos2(right, staff_y), pos2(right, staff_y + 8.8)],
                    Stroke::new(0.85, margin),
                );
                staff_y += 16.5;
            }
        }
        PaperTemplate::WeeklyPlanner => {
            let head_y = top + 5.0;
            rp.line_segment(
                [pos2(left, head_y), pos2(right, head_y)],
                Stroke::new(1.0, margin),
            );
            let box_rect = Rect::from_min_max(pos2(left, head_y + 3.0), pos2(right, bottom));
            rp.rect_stroke(box_rect, 1.0, Stroke::new(0.85, margin), StrokeKind::Inside);
            rp.line_segment(
                [
                    pos2(box_rect.center().x, box_rect.top()),
                    pos2(box_rect.center().x, box_rect.bottom()),
                ],
                Stroke::new(0.75, margin),
            );
            for r in 1..4 {
                let y = box_rect.top() + box_rect.height() * (r as f32 / 4.0);
                rp.line_segment([pos2(left, y), pos2(right, y)], Stroke::new(0.75, line));
            }
        }
        PaperTemplate::MonthlyPlanner => {
            let head_y = top + 5.0;
            rp.line_segment(
                [pos2(left, head_y), pos2(right, head_y)],
                Stroke::new(1.0, margin),
            );
            let grid_bottom = bottom - 9.0;
            let cw = (right - left) / 7.0;
            for c in 0..=7 {
                let x = left + c as f32 * cw;
                rp.line_segment(
                    [pos2(x, head_y + 3.0), pos2(x, grid_bottom)],
                    Stroke::new(0.65, line),
                );
            }
            let rh = (grid_bottom - (head_y + 3.0)) / 5.0;
            for r in 0..=5 {
                let y = head_y + 3.0 + r as f32 * rh;
                rp.line_segment([pos2(left, y), pos2(right, y)], Stroke::new(0.65, line));
            }
            rp.line_segment(
                [pos2(left, bottom - 3.0), pos2(right, bottom - 3.0)],
                Stroke::new(0.75, line),
            );
        }
        PaperTemplate::Checklist => {
            let head_y = top + 5.0;
            rp.line_segment(
                [pos2(left, head_y), pos2(right, head_y)],
                Stroke::new(1.0, margin),
            );
            let div_x = left + 8.0;
            rp.line_segment(
                [pos2(div_x, head_y), pos2(div_x, bottom)],
                Stroke::new(0.85, margin),
            );
            let mut y = head_y + 7.5;
            while y < bottom {
                rp.line_segment([pos2(left, y), pos2(right, y)], Stroke::new(0.7, line));
                let bx = Rect::from_min_size(pos2(left + 1.5, y - 5.5), Vec2::splat(4.0));
                rp.rect_stroke(bx, 0.5, Stroke::new(0.75, margin), StrokeKind::Inside);
                y += 7.5;
            }
        }
    }

    rp.rect_stroke(
        unrot_rect,
        4.0,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
}

fn paint_horizontal_rules(
    p: &PreviewPainter,
    left: f32,
    right: f32,
    start_y: f32,
    end_y: f32,
    step: f32,
    color: Color32,
) {
    let mut y = start_y;
    while y < end_y {
        p.line_segment([pos2(left, y), pos2(right, y)], Stroke::new(0.8, color));
        y += step;
    }
}

struct NotebookHeroSpec {
    title_id_salt: &'static str,
    title_hint: &'static str,
    summary: String,
    cover: NotebookCover,
    first_page: (PaperTemplate, PaperColor),
    page_dimensions: (u32, u32),
    theme: UiTheme,
    auto_contrast: bool,
}

/// Renders the unified studio-style New Notebook modal:
/// side-by-side clickable `COVER` and `PAPER` preview cards, clean right-hand title / size /
/// paper-color controls, and the matching template shelf below. Always creates directly inside
/// the current folder (`app.view.new_notebook_folder`).
pub(crate) fn render_new_notebook_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let active_tab = app.view.new_notebook_preview_tab.min(1); // 0 = Paper, 1 = Cover
    let (page_w, page_h) = app.view.new_notebook_format.effective_dimensions();
    let paper_colors = app.view.new_notebook_color.resolved_colors(
        app.session.prefs.theme,
        app.session.prefs.auto_contrast_with_theme,
    );

    // Top strip: [COVER card] [PAPER card]  |  [Title, Size/Orientation, Paper Color]
    // Flat on the dialog surface: no tinted panel, no eyebrow labels, no boxed settings card.
    egui::Frame::NONE.inner_margin(Margin::ZERO).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;

            // 1. Clickable COVER preview card
            let cover_label = app.view.new_notebook_cover.display_label();
            if draw_studio_preview_tab_card(
                ui,
                "new_nb_tab_cover",
                "COVER",
                &cover_label,
                active_tab == 1,
                t,
                |p, r| {
                    paint_hardcover_book_cover(p, r, app.view.new_notebook_cover, None, t.border);
                },
            )
            .clicked()
            {
                app.view.new_notebook_preview_tab = 1;
            }

            // 2. Clickable PAPER preview card
            let paper_label = app
                .view
                .new_notebook_custom_template_id
                .as_deref()
                .and_then(|id| {
                    app.session
                        .prefs
                        .custom_templates
                        .iter()
                        .find(|tpl| tpl.id == id)
                        .map(|tpl| tpl.name.as_str())
                })
                .unwrap_or_else(|| app.view.new_notebook_template.label());
            if draw_studio_preview_tab_card(
                ui,
                "new_nb_tab_paper",
                "PAPER",
                paper_label,
                active_tab == 0,
                t,
                |p, r| {
                    let aspect = (page_w as f32 / page_h.max(1) as f32).clamp(0.58, 1.45);
                    let fit_r = fit_rect_aspect(r, aspect);
                    paint_paper_preview_colors(
                        p,
                        fit_r,
                        app.view.new_notebook_template,
                        paper_colors,
                        t.border,
                    );
                },
            )
            .clicked()
            {
                app.view.new_notebook_preview_tab = 0;
            }

            // 3. Right-hand clean controls (Title, Size + Orientation, Paper Color)
            let right_w = ui.available_width().max(180.0);
            ui.allocate_ui_with_layout(
                Vec2::new(right_w, 168.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right_w);
                    ui.spacing_mut().item_spacing.y = 8.0;

                    ui.add_sized(
                        [right_w, 32.0],
                        egui::TextEdit::singleline(&mut app.view.new_notebook_title)
                            .id(ui.make_persistent_id("new_notebook_title_input"))
                            .hint_text("Untitled Notebook")
                            .margin(Margin::symmetric(10, 4))
                            .align(egui::Align2::LEFT_CENTER),
                    );

                    // Size + Orientation + Paper Color as two plain rows split by a hairline.
                    egui::Frame::NONE
                        .inner_margin(Margin::symmetric(0, 4))
                        .show(ui, |ui| {
                            ui.set_width(right_w);
                            ui.spacing_mut().item_spacing.y = 8.0;

                            // Row 1: Size + Portrait/Landscape toggle
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Size").size(12.0).color(t.text));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        let orient_label =
                                            match app.view.new_notebook_format.orientation {
                                                PageOrientation::Portrait => "Portrait",
                                                PageOrientation::Landscape => "Landscape",
                                            };
                                        if compact_icon_button(
                                            ui,
                                            Icon::RotateCw,
                                            Some(orient_label),
                                            false,
                                            true,
                                            t,
                                        )
                                        .on_hover_text("Toggle Portrait / Landscape")
                                        .clicked()
                                        {
                                            app.view.new_notebook_format.orientation =
                                                match app.view.new_notebook_format.orientation {
                                                    PageOrientation::Portrait => {
                                                        PageOrientation::Landscape
                                                    }
                                                    PageOrientation::Landscape => {
                                                        PageOrientation::Portrait
                                                    }
                                                };
                                        }
                                        let mut preset = app.view.new_notebook_format.preset;
                                        crate::theme::studio_combo_box(
                                            ui,
                                            "new_nb_size_combo",
                                            RichText::new(preset.label()).size(11.5).color(t.text),
                                            96.0,
                                            |ui| {
                                                for &opt in PageResolutionPreset::ALL {
                                                    ui.selectable_value(
                                                        &mut preset,
                                                        opt,
                                                        opt.label(),
                                                    );
                                                }
                                            },
                                        );
                                        if preset != app.view.new_notebook_format.preset {
                                            app.view.new_notebook_format.preset = preset;
                                        }
                                    },
                                );
                            });

                            // Hairline divider
                            let (hr, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 1.0),
                                Sense::hover(),
                            );
                            ui.painter().line_segment(
                                [hr.left_center(), hr.right_center()],
                                Stroke::new(1.0, t.border),
                            );

                            // Row 2: Paper Color quick dots (Yellow/Cream, White, Dark)
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Color").size(12.0).color(t.text));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        draw_sheet_tint_strip(
                                            ui,
                                            &mut app.view.new_notebook_color,
                                            t,
                                        );
                                    },
                                );
                            });
                        });

                    if !app.view.new_notebook_folder.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "In folder: {}",
                                crate::library_folders::display_folder_path(
                                    &app.view.new_notebook_folder
                                )
                            ))
                            .size(11.0)
                            .color(t.text_muted),
                        );
                    }
                },
            );
        });
    });

    ui.add_space(10.0);
    let (hr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment(
        [hr.left_center(), hr.right_center()],
        Stroke::new(1.0, t.border),
    );
    ui.add_space(12.0);

    if active_tab == 1 {
        ui.label(
            RichText::new("Cover Templates")
                .family(egui::FontFamily::Name(crate::theme::DISPLAY_FAMILY.into()))
                .size(14.0)
                .color(t.text),
        );
        ui.add_space(8.0);
        draw_cover_shelf_and_custom_picker(
            ui,
            &mut app.view.new_notebook_cover,
            &mut app.view.cover_color_draft,
            t,
        );
    } else {
        ui.label(
            RichText::new("Paper Templates")
                .family(egui::FontFamily::Name(crate::theme::DISPLAY_FAMILY.into()))
                .size(14.0)
                .color(t.text),
        );
        ui.add_space(8.0);
        draw_paper_sheet_grid_for(
            ui,
            PaperGridTarget {
                id_salt: "new_nb_sheet_tile",
                theme: app.session.prefs.theme,
                auto_contrast: app.session.prefs.auto_contrast_with_theme,
                template: &mut app.view.new_notebook_template,
                color: app.view.new_notebook_color,
                custom_template_id: &mut app.view.new_notebook_custom_template_id,
                clear_custom: None,
            },
            t,
        );
        ui.add_space(10.0);
        draw_custom_templates_section(app, ui, TemplatePickerTarget::NewNotebook, t);
    }
    false
}

fn fit_rect_aspect(bounds: Rect, aspect: f32) -> Rect {
    let bw = bounds.width();
    let bh = bounds.height();
    let (w, h) = if bw / bh.max(1.0) > aspect {
        (bh * aspect, bh)
    } else {
        (bw, bw / aspect.max(0.1))
    };
    Rect::from_center_size(bounds.center(), Vec2::new(w, h))
}

fn draw_studio_preview_tab_card(
    ui: &mut egui::Ui,
    id_salt: &str,
    eyebrow: &str,
    label: &str,
    selected: bool,
    t: &Tokens,
    paint_preview: impl FnOnce(&egui::Painter, Rect),
) -> egui::Response {
    let card_size = Vec2::new(112.0, 168.0);
    let (rect, resp) = ui.allocate_exact_size(card_size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hover_t = crate::motion::hover_factor(
            ui.ctx(),
            ui.make_persistent_id(id_salt).with("hov"),
            resp.hovered() && !selected,
        );
        let fill = if selected {
            t.field_bg
        } else {
            crate::motion::lerp_color(Color32::TRANSPARENT, t.ghost_hover, hover_t)
        };
        if fill.a() > 0 {
            p.rect_filled(rect, CornerRadius::same(12), fill);
        }
        p.text(
            pos2(rect.center().x, rect.top() + 14.0),
            egui::Align2::CENTER_CENTER,
            eyebrow,
            egui::FontId::proportional(10.0),
            t.text_muted,
        );
        let preview_rect = Rect::from_center_size(
            pos2(rect.center().x, rect.top() + 82.0),
            Vec2::new(82.0, 108.0),
        );
        paint_preview(p, preview_rect);
        let caption_color = if selected { t.text } else { t.text_muted };
        p.text(
            pos2(rect.center().x, rect.bottom() - 18.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.5),
            caption_color,
        );
        if selected {
            let bar = Rect::from_center_size(
                pos2(rect.center().x, rect.bottom() - 4.0),
                Vec2::new(42.0, 3.0),
            );
            p.rect_filled(bar, CornerRadius::same(2), t.accent);
        }
    }
    resp
}

/// Renders the New Notebook dialog footer (`Cancel` + `Create Notebook` directly on one screen).
pub(crate) fn render_new_notebook_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let mut close = false;
    let mut create_clicked = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if primary_action_button(ui, "Create Notebook", true, t).clicked() {
                create_clicked = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                close = true;
            }
        });
    });
    if create_clicked {
        let trimmed = app.view.new_notebook_title.trim();
        let (page_w, page_h) = app.view.new_notebook_format.effective_dimensions();
        let mut payload = json!({
            "preset": app.view.new_notebook_preset,
            "cover": app.view.new_notebook_cover,
            "template": app.view.new_notebook_template,
            "paper_color": app.view.new_notebook_color,
            "folder_path": app.view.new_notebook_folder,
            "favorite": app.view.new_notebook_favorite,
            "tags": app.view.new_notebook_tags,
            "resolution": app.view.new_notebook_format.preset,
            "orientation": app.view.new_notebook_format.orientation,
            "width": page_w,
            "height": page_h,
        });
        if !trimmed.is_empty() {
            payload["title"] = json!(trimmed);
        }
        if let Some(ref tpl_id) = app.view.new_notebook_custom_template_id {
            payload["custom_template_id"] = json!(tpl_id);
        }
        let _ = app.session.execute("notebook.create", &payload);
        crate::sync_view_to_active_notebook(app);
        app.view.inspector_open = false;
        app.view.screen = crate::AppScreen::Editor;
        return true;
    }
    close
}

/// Renders the unified Notebook Settings modal body (`Details & Cover`, `Size & Orientation`,
/// `Paper & Templates`). Returns `true` when the modal should close.
pub(crate) fn render_notebook_settings_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let tab = app.view.notebook_settings_step.min(2);
    let page_count = app.session.doc.page_count();
    let noun = if page_count == 1 { "page" } else { "pages" };
    let (eff_w, eff_h) = page_setup_effective_dims(app);
    let turns = app.view.page_setup_quarter_turns.rem_euclid(4);
    let style_label = page_setup_style_label(app);
    let summary = format!(
        "{page_count} {noun}  •  {}  •  {style_label}  •  {eff_w} × {eff_h} pt",
        app.view.notebook_cover_draft.display_label(),
    );

    match tab {
        0 => {
            let hero_spec = NotebookHeroSpec {
                title_id_salt: "notebook_settings_title_input",
                title_hint: "Enter notebook name...",
                summary,
                cover: app.view.notebook_cover_draft,
                first_page: (app.view.page_setup_template, app.view.page_setup_color),
                page_dimensions: (eff_w, eff_h),
                theme: app.session.prefs.theme,
                auto_contrast: app.session.prefs.auto_contrast_with_theme,
            };
            draw_notebook_hero_card(ui, &hero_spec, &mut app.view.notebook_title_draft, t);

            ui.add_space(10.0);
            draw_modal_section_title(
                ui,
                "Folder, Favorite & Tags",
                "Organize into folders, star as favorite, or click any tag pill to edit inline.",
                t,
            );
            let known_folders = known_library_folders(app);
            let library_tags = library_tags_with_counts(app);
            draw_notebook_organization_section(
                ui,
                &app.session.prefs,
                &known_folders,
                &library_tags,
                OrganizationDrafts {
                    id_prefix: "nb_settings",
                    folder: &mut app.view.notebook_folder_draft,
                    folder_new_open: &mut app.view.folder_picker_new_open,
                    folder_new_name: &mut app.view.folder_picker_new_name,
                    favorite: &mut app.view.notebook_favorite_draft,
                    tags: &mut app.view.notebook_tags_draft,
                    tag_input: &mut app.view.notebook_tag_input_draft,
                    tag_edit_index: &mut app.view.tag_edit_index,
                    tag_edit_draft: &mut app.view.tag_edit_draft,
                    tag_edit_focus_pending: &mut app.view.tag_edit_focus_pending,
                },
                t,
            );

            ui.add_space(10.0);
            draw_modal_section_title(
                ui,
                "Book Cover",
                "Choose a hardcover finish or pick any custom cover color.",
                t,
            );
            draw_cover_shelf_and_custom_picker(
                ui,
                &mut app.view.notebook_cover_draft,
                &mut app.view.cover_color_draft,
                t,
            );
        }
        1 => {
            draw_page_setup_scope_strip(app, ui, t);
            ui.add_space(10.0);
            let paper_colors = app.view.page_setup_color.resolved_colors(
                app.session.prefs.theme,
                app.session.prefs.auto_contrast_with_theme,
            );
            let custom_backdrop_active = !app.view.page_setup_clear_custom
                && (app.view.page_setup_custom_template_id.is_some()
                    || app.session.doc.pdf_backdrop.is_some()
                    || app.session.doc.custom_image_backdrop.is_some());
            let mut preview_ctx = PageFormatPreviewContext {
                id_prefix: "page_setup_format",
                paper_colors,
                paper_template: app.view.page_setup_template,
                preview_turns: turns,
                custom_backdrop_active,
                rotate_turns: Some(&mut app.view.page_setup_quarter_turns),
            };
            render_page_format_picker(ui, t, &mut app.view.page_setup_format, &mut preview_ctx);
        }
        _ => {
            draw_page_setup_scope_strip(app, ui, t);
            ui.add_space(10.0);
            draw_page_setup_paper_preview_banner(app, ui, t);
            ui.add_space(10.0);
            draw_paper_section_header_with_tint(
                ui,
                "Paper Ruling & Tint",
                "Pick a ruling sheet and paper tint to preview on the page.",
                &mut app.view.page_setup_color,
                t,
            );

            ui.add_space(8.0);
            draw_paper_sheet_grid_for(
                ui,
                PaperGridTarget {
                    id_salt: "page_setup_sheet_tile",
                    theme: app.session.prefs.theme,
                    auto_contrast: app.session.prefs.auto_contrast_with_theme,
                    template: &mut app.view.page_setup_template,
                    color: app.view.page_setup_color,
                    custom_template_id: &mut app.view.page_setup_custom_template_id,
                    clear_custom: Some(&mut app.view.page_setup_clear_custom),
                },
                t,
            );

            ui.add_space(10.0);
            draw_custom_templates_section(app, ui, TemplatePickerTarget::NotebookSettings, t);
        }
    }
    false
}

/// Renders the unified Notebook Settings modal footer. Returns `true` when the modal should close.
pub(crate) fn render_notebook_settings_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let tab = app.view.notebook_settings_step.min(2);
    let page_count = app.session.doc.page_count().max(1);
    let apply_all = app.view.page_setup_apply_all && page_count > 1;
    let turns = app.view.page_setup_quarter_turns.rem_euclid(4);
    let (eff_w, eff_h) = page_setup_effective_dims(app);
    let save_label = if apply_all && tab > 0 {
        "Apply to All Pages"
    } else {
        "Save Changes"
    };

    let mut cancel_clicked = false;
    let mut save_clicked = false;
    let mut add_page_clicked = false;

    ui.horizontal(|ui| {
        if tab == 0 {
            if danger_action_button(ui, "Move to Trash", true, t)
                .on_hover_text("Move this notebook to Trash, where it stays for 30 days")
                .clicked()
            {
                app.view.pending_delete_notebook = app
                    .session
                    .notebooks
                    .get(app.session.active_notebook)
                    .cloned();
                modal::open(app, modal::ActiveModal::ConfirmDeleteNotebook);
            }
            if app.session.notebooks.len() > 1
                && secondary_action_button(ui, "Merge...", true, t)
                    .on_hover_text("Append all pages of this notebook into another notebook")
                    .clicked()
            {
                app.view.transfer_merge_whole_notebook = Some(app.session.active_notebook);
                app.view.transfer_merge_delete_source = false;
                modal::open(app, modal::ActiveModal::TransferPages);
            }
        } else if secondary_action_button(ui, "+ Add Page with Style", true, t)
            .on_hover_text(
                "Insert a new page right after the active page using the selected size, orientation, and paper style",
            )
            .clicked()
        {
            add_page_clicked = true;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if primary_action_button(ui, save_label, true, t).clicked() {
                save_clicked = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                cancel_clicked = true;
            }
            if tab > 0 && page_count > 1 {
                ui.add_space(4.0);
                let active_num = (app.session.doc.active_page_index() + 1).min(page_count);
                let single_tip =
                    format!("Apply size, style, and orientation only to Page {active_num}");
                let all_label = format!("All {page_count} Pages");
                let scope_options = [
                    SwitchOption::new(None, Some("This Page Only"), &single_tip, true),
                    SwitchOption::new(
                        None,
                        Some(&all_label),
                        "Apply sheet size, ruling, tint, template, and orientation to every page",
                        true,
                    ),
                ];
                let (scope_clicked, _) = segmented_switch(
                    ui,
                    "page_setup_apply_scope_switch",
                    &scope_options,
                    Some(usize::from(app.view.page_setup_apply_all)),
                    SwitchLayout::Compact,
                    t,
                );
                if let Some(idx) = scope_clicked {
                    app.view.page_setup_apply_all = idx == 1;
                }
                crate::theme::studio_row_label(
                    ui,
                    RichText::new("Apply to:")
                        .size(11.5)
                        .strong()
                        .color(t.text_muted),
                    28.0,
                );
            }
        });
    });

    if cancel_clicked {
        return true;
    }

    if add_page_clicked {
        crate::canvas::commit_pending_text(app);
        let (req_w, req_h) = if turns % 2 == 1 && app.view.page_setup_custom_template_id.is_some() {
            (eff_h, eff_w)
        } else {
            (eff_w, eff_h)
        };
        let mut payload = json!({
            "template": app.view.page_setup_template,
            "color": app.view.page_setup_color,
            "width": req_w,
            "height": req_h,
        });
        if turns == 0 || app.view.page_setup_custom_template_id.is_none() {
            payload["resolution"] = json!(app.view.page_setup_format.preset);
            payload["orientation"] = json!(app.view.page_setup_format.orientation);
        }
        if let Some(ref tpl_id) = app.view.page_setup_custom_template_id {
            payload["custom_template_id"] = json!(tpl_id);
        }
        let _ = app.session.execute("page.add", &payload);
        if turns != 0 && app.view.page_setup_custom_template_id.is_some() {
            let _ = app
                .session
                .execute("page.rotate", &json!({ "quarter_turns": turns }));
        }
        app.mesh_cache.clear();
        app.view.fit_requested = true;
        return true;
    }

    if save_clicked {
        crate::canvas::commit_pending_text(app);

        // 1. Notebook identity & cover
        let trimmed_title = app.view.notebook_title_draft.trim().to_string();
        let title_changed = !trimmed_title.is_empty() && trimmed_title != app.session.doc.title;
        let cover_changed = app.view.notebook_cover_draft != app.session.doc.cover;
        if title_changed || cover_changed {
            let mut rename_payload = json!({
                "cover": app.view.notebook_cover_draft,
            });
            if !trimmed_title.is_empty() {
                rename_payload["title"] = json!(trimmed_title);
            }
            let _ = app.session.execute("notebook.rename", &rename_payload);
        }

        // 2. Folder, favorite & inline tags (commit any pending input/edit first)
        commit_active_tag_edit(
            &mut app.view.notebook_tags_draft,
            &mut app.view.tag_edit_index,
            &mut app.view.tag_edit_draft,
            &mut app.view.tag_edit_focus_pending,
        );
        commit_tag_input_tokens(
            &app.view.notebook_tag_input_draft,
            &mut app.view.notebook_tags_draft,
        );
        app.view.notebook_tag_input_draft.clear();

        let known_folders = known_library_folders(app);
        commit_pending_picker_folder(
            &known_folders,
            FolderPickerDrafts {
                folder: &mut app.view.notebook_folder_draft,
                new_open: &mut app.view.folder_picker_new_open,
                new_name: &mut app.view.folder_picker_new_name,
            },
        );
        let next_tags = drawcraft_doc::sanitize_notebook_tags(&app.view.notebook_tags_draft);
        app.view.notebook_tags_draft = next_tags.clone();
        let metadata_changed = app.view.notebook_folder_draft != app.session.doc.folder_path
            || app.view.notebook_favorite_draft != app.session.doc.favorite
            || next_tags != app.session.doc.tags;
        if metadata_changed {
            let _ = app.session.execute(
                "notebook.setMetadata",
                &json!({
                    "folder_path": app.view.notebook_folder_draft,
                    "favorite": app.view.notebook_favorite_draft,
                    "tags": next_tags,
                }),
            );
        }

        // 3. Page template / tint / custom backdrop / rotation / dimensions
        let prev_dims = (app.session.doc.width, app.session.doc.height);
        let template_changed = apply_all
            || app.view.page_setup_template != app.session.doc.paper_template
            || app.view.page_setup_color != app.session.doc.paper_color
            || app.view.page_setup_custom_template_id != app.session.doc.custom_template_id
            || app.view.page_setup_clear_custom;
        if template_changed {
            let cmd = if apply_all {
                "page.applyTemplateAll"
            } else {
                "page.setTemplate"
            };
            let tpl_payload = if let Some(ref tpl_id) = app.view.page_setup_custom_template_id {
                json!({
                    "custom_template_id": tpl_id,
                    "color": app.view.page_setup_color,
                })
            } else {
                let mut p = json!({
                    "template": app.view.page_setup_template,
                    "color": app.view.page_setup_color,
                });
                if app.view.page_setup_clear_custom {
                    p["clear_custom_template"] = json!(true);
                }
                p
            };
            let _ = app.session.execute(cmd, &tpl_payload);
            app.mesh_cache.clear();
        }

        if turns != 0 {
            let rot_payload = if apply_all {
                let all_pages: Vec<usize> = (0..page_count).collect();
                json!({ "pages": all_pages, "quarter_turns": turns })
            } else {
                json!({ "quarter_turns": turns })
            };
            let _ = app.session.execute("page.rotate", &rot_payload);
            app.mesh_cache.clear();
            app.view.fit_requested = true;
        }

        let needs_size_update = (app.session.doc.width, app.session.doc.height) != (eff_w, eff_h)
            || (apply_all
                && app
                    .session
                    .doc
                    .pages
                    .iter()
                    .any(|pg| (pg.width, pg.height) != (eff_w, eff_h)));
        if needs_size_update {
            let _ = app.session.execute(
                "page.setSize",
                &json!({
                    "apply_all": apply_all,
                    "resolution": app.view.page_setup_format.preset,
                    "orientation": app.view.page_setup_format.orientation,
                    "width": eff_w,
                    "height": eff_h,
                }),
            );
            app.mesh_cache.clear();
        }
        if (app.session.doc.width, app.session.doc.height) != prev_dims {
            app.view.fit_requested = true;
        }

        return true;
    }

    false
}

struct OrganizationDrafts<'a> {
    id_prefix: &'static str,
    folder: &'a mut String,
    folder_new_open: &'a mut bool,
    folder_new_name: &'a mut String,
    favorite: &'a mut bool,
    tags: &'a mut Vec<String>,
    tag_input: &'a mut String,
    tag_edit_index: &'a mut Option<usize>,
    tag_edit_draft: &'a mut String,
    tag_edit_focus_pending: &'a mut bool,
}

fn commit_tag_input_tokens(raw: &str, tags: &mut Vec<String>) {
    for part in raw.split([',', ';']) {
        if let Some(clean) = drawcraft_doc::sanitize_notebook_tag(part) {
            tags.push(clean);
        }
    }
    *tags = drawcraft_doc::sanitize_notebook_tags(tags);
}

fn commit_active_tag_edit(
    tags: &mut Vec<String>,
    edit_index: &mut Option<usize>,
    edit_draft: &mut String,
    focus_pending: &mut bool,
) {
    let Some(idx) = edit_index.take() else {
        return;
    };
    *focus_pending = false;
    let draft = std::mem::take(edit_draft);
    if idx < tags.len() {
        if let Some(clean) = drawcraft_doc::sanitize_notebook_tag(&draft) {
            tags[idx] = clean;
        } else {
            tags.remove(idx);
        }
        *tags = drawcraft_doc::sanitize_notebook_tags(tags);
    }
}

fn draw_notebook_organization_section(
    ui: &mut egui::Ui,
    prefs: &drawcraft_doc::UserStudioConfig,
    known_folders: &[String],
    library_tags: &[(String, usize)],
    drafts: OrganizationDrafts<'_>,
    t: &Tokens,
) {
    let OrganizationDrafts {
        id_prefix,
        folder,
        folder_new_open,
        folder_new_name,
        favorite,
        tags,
        tag_input,
        tag_edit_index,
        tag_edit_draft,
        tag_edit_focus_pending,
    } = drafts;

    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Row 1: Folder picker on the left, Favorite toggle right-aligned on the same bar.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 150.0).max(240.0));
                    draw_folder_picker(
                        ui,
                        &format!("{id_prefix}_folder_combo"),
                        known_folders,
                        prefs,
                        FolderPickerDrafts {
                            folder,
                            new_open: folder_new_open,
                            new_name: folder_new_name,
                        },
                        t,
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let fav = *favorite;
                    if icon_button(
                        ui,
                        Icon::Star,
                        Some(if fav { "Favorite" } else { "Mark Favorite" }),
                        fav,
                        true,
                        t,
                    )
                    .on_hover_text("Pin this notebook in your Favorites filter on the Home library")
                    .clicked()
                    {
                        *favorite = !fav;
                    }
                });
            });

            // Row 2: Existing inline-editable tag pills (click pill text to edit inline, X to remove).
            if !tags.is_empty() {
                ui.add_space(6.0);
                let mut remove_idx = None;
                let mut commit_edit = false;
                let mut cancel_edit = false;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 5.0);
                    for (idx, tag_ref) in tags.iter().enumerate() {
                        let is_editing = *tag_edit_index == Some(idx);
                        if is_editing {
                            egui::Frame::NONE
                                .fill(t.field_bg)
                                .stroke(Stroke::new(1.2, t.accent))
                                .corner_radius(CornerRadius::same(9))
                                .inner_margin(Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        ui.label(
                                            RichText::new("#").size(11.0).strong().color(t.accent),
                                        );
                                        let edit_resp = ui.add_sized(
                                            [106.0, 22.0],
                                            egui::TextEdit::singleline(tag_edit_draft)
                                                .id_source((id_prefix, "tag_inline_edit", idx))
                                                .margin(egui::Margin::symmetric(4, 1))
                                                .align(egui::Align2::LEFT_CENTER)
                                                .hint_text("Edit tag..."),
                                        );
                                        if *tag_edit_focus_pending {
                                            edit_resp.request_focus();
                                            *tag_edit_focus_pending = false;
                                        }
                                        let pressed_enter = edit_resp.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                        let pressed_escape = edit_resp.has_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Escape));
                                        if compact_icon_button(ui, Icon::Check, None, true, true, t)
                                            .on_hover_text("Save tag rename")
                                            .clicked()
                                            || pressed_enter
                                        {
                                            commit_edit = true;
                                        }
                                        if chip_close_button(
                                            ui,
                                            "Cancel editing tag",
                                            t.text_muted,
                                            t,
                                        )
                                        .clicked()
                                            || pressed_escape
                                        {
                                            cancel_edit = true;
                                        }
                                    });
                                });
                        } else {
                            let tag_text = tag_ref.clone();
                            let tag_color = prefs.tag_color(&tag_text);
                            let vis = crate::library_folders::folder_visual_colors(tag_color, t);
                            let border_color = if tag_color.is_some() {
                                vis.icon
                            } else {
                                t.accent
                            };
                            egui::Frame::NONE
                                .fill(vis.soft_fill)
                                .stroke(Stroke::new(1.0, border_color))
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(Margin::symmetric(8, 3))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        if ui
                                            .add(
                                                egui::Label::new(
                                                    RichText::new(format!("#{tag_text}"))
                                                        .size(11.0)
                                                        .strong()
                                                        .color(vis.selected_text),
                                                )
                                                .sense(Sense::click()),
                                            )
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text("Click to rename this tag inline")
                                            .clicked()
                                        {
                                            *tag_edit_index = Some(idx);
                                            *tag_edit_draft = tag_text;
                                            *tag_edit_focus_pending = true;
                                        }
                                        if chip_close_button(ui, "Remove tag", vis.selected_text, t)
                                            .clicked()
                                        {
                                            remove_idx = Some(idx);
                                        }
                                    });
                                });
                        }
                    }
                });
                if commit_edit {
                    commit_active_tag_edit(
                        tags,
                        tag_edit_index,
                        tag_edit_draft,
                        tag_edit_focus_pending,
                    );
                } else if cancel_edit {
                    *tag_edit_index = None;
                    tag_edit_draft.clear();
                    *tag_edit_focus_pending = false;
                }
                if let Some(rm_idx) = remove_idx {
                    tags.remove(rm_idx);
                    if let Some(cur_edit) = *tag_edit_index {
                        if cur_edit == rm_idx {
                            *tag_edit_index = None;
                            tag_edit_draft.clear();
                            *tag_edit_focus_pending = false;
                        } else if cur_edit > rm_idx {
                            *tag_edit_index = Some(cur_edit - 1);
                        }
                    }
                }
            }

            // Row 3: Tag entry input + quick 1-click library tag suggestions.
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let enter = ui
                    .add_sized(
                        [210.0, 26.0],
                        egui::TextEdit::singleline(tag_input)
                            .id_source((id_prefix, "tag_add_input"))
                            .margin(egui::Margin::symmetric(8, 2))
                            .align(egui::Align2::LEFT_CENTER)
                            .hint_text("Add tags (comma-separated)..."),
                    )
                    .lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (compact_icon_button(ui, Icon::Plus, Some("Add Tag"), false, true, t).clicked()
                    || enter)
                    && !tag_input.trim().is_empty()
                {
                    commit_tag_input_tokens(tag_input, tags);
                    tag_input.clear();
                }

                let unused_suggestions: Vec<&str> = library_tags
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .filter(|cand| !tags.iter().any(|existing| existing == cand))
                    .take(4)
                    .collect();
                if !unused_suggestions.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Suggestions:").size(10.5).color(t.text_muted));
                    for suggestion in unused_suggestions {
                        let sug_color = prefs.tag_color(suggestion);
                        let sug_border = sug_color.map_or(t.border, |c| {
                            crate::library_folders::folder_visual_colors(Some(c), t).icon
                        });
                        let resp = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("+#{suggestion}"))
                                        .size(10.5)
                                        .color(t.text_muted),
                                )
                                .fill(t.toolbar_bg)
                                .stroke(Stroke::new(1.0, sug_border))
                                .corner_radius(CornerRadius::same(8))
                                .min_size(Vec2::new(0.0, 22.0)),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Click to add existing library tag");
                        if resp.clicked() {
                            tags.push(suggestion.to_string());
                            *tags = drawcraft_doc::sanitize_notebook_tags(tags);
                        }
                    }
                }
            });
        });
}

/// Renders the Transfer Pages or Merge Notebooks modal body.
pub(crate) fn render_transfer_pages_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    ui.set_min_height(156.0);
    let notebook_count = app.session.notebooks.len();
    let active_nb = app.session.active_notebook;

    if let Some(source_idx) = app.view.transfer_merge_whole_notebook {
        let src_title = app
            .session
            .notebooks
            .get(source_idx)
            .map_or("Notebook", |d| d.title.as_str())
            .to_string();
        let src_pages = app
            .session
            .notebooks
            .get(source_idx)
            .map_or(0, |d| d.page_count());
        ui.label(
            RichText::new(format!(
                "Append all {src_pages} pages of \"{src_title}\" onto another notebook."
            ))
            .size(12.5)
            .color(t.text),
        );
        ui.add_space(10.0);
        if app.view.transfer_target_notebook == source_idx && notebook_count > 1 {
            app.view.transfer_target_notebook =
                (0..notebook_count).find(|&i| i != source_idx).unwrap_or(0);
        }
        draw_destination_notebook_picker(app, ui, Some(source_idx), t);
        ui.add_space(8.0);
        ui.checkbox(
            &mut app.view.transfer_merge_delete_source,
            "Move merged source notebook to Trash after merging",
        );
        return false;
    }

    let valid_count = app.view.transfer_pages_list.len();
    ui.label(
        RichText::new(format!(
            "Transferring {valid_count} {} from \"{}\".",
            if valid_count == 1 { "page" } else { "pages" },
            app.session.doc.title
        ))
        .size(12.5)
        .color(t.text),
    );
    ui.add_space(8.0);
    let can_move = valid_count > 0 && valid_count < app.session.doc.page_count();
    if !can_move {
        app.view.transfer_move_mode = false;
    }
    let transfer_mode_options = [
        SwitchOption::new(None, Some("Copy pages"), "Duplicate selected pages", true),
        SwitchOption::new(
            None,
            Some("Move pages"),
            "Move selected pages to the destination notebook",
            can_move,
        ),
    ];
    let (mode_clicked, _) = segmented_switch(
        ui,
        "transfer_pages_mode_switch",
        &transfer_mode_options,
        Some(usize::from(app.view.transfer_move_mode)),
        SwitchLayout::EqualWidth,
        t,
    );
    if let Some(idx) = mode_clicked {
        app.view.transfer_move_mode = idx == 1 && can_move;
    }
    if !can_move {
        ui.label(
            RichText::new(
                "A notebook must keep at least one page, so moving all pages switches to Copy.",
            )
            .size(11.0)
            .color(t.text_muted),
        );
    }

    ui.add_space(10.0);
    let has_other = notebook_count > 1;
    if !has_other {
        app.view.transfer_create_new_notebook = true;
    }
    let destination_options = [
        SwitchOption::new(
            None,
            Some("Existing notebook"),
            "Send selected pages into an existing notebook",
            has_other,
        ),
        SwitchOption::new(
            None,
            Some("Create new notebook"),
            "Create a new notebook for the selected pages",
            true,
        ),
    ];
    let (dest_clicked, _) = segmented_switch(
        ui,
        "transfer_pages_destination_switch",
        &destination_options,
        Some(usize::from(app.view.transfer_create_new_notebook)),
        SwitchLayout::EqualWidth,
        t,
    );
    if let Some(idx) = dest_clicked {
        app.view.transfer_create_new_notebook = idx == 1 || !has_other;
    }
    ui.add_space(8.0);
    let dest_subflow = u8::from(app.view.transfer_create_new_notebook);
    let dest_order = isize::from(app.view.transfer_create_new_notebook);
    modal::apply_subflow_motion(
        app,
        ui,
        modal::ActiveModal::TransferPages,
        dest_subflow,
        dest_order,
        |app, ui| {
            if app.view.transfer_create_new_notebook {
                ui.add_sized(
                    [ui.available_width(), 28.0],
                    egui::TextEdit::singleline(&mut app.view.transfer_new_title)
                        .margin(egui::Margin::symmetric(8, 2))
                        .align(egui::Align2::LEFT_CENTER)
                        .hint_text("Enter new notebook name..."),
                );
            } else {
                let exclude = if app.view.transfer_move_mode {
                    Some(active_nb)
                } else {
                    None
                };
                draw_destination_notebook_picker(app, ui, exclude, t);
            }
        },
    );
    false
}

fn draw_destination_notebook_picker(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    exclude_idx: Option<usize>,
    t: &Tokens,
) {
    if Some(app.view.transfer_target_notebook) == exclude_idx
        && let Some(first_ok) = (0..app.session.notebooks.len()).find(|&i| Some(i) != exclude_idx)
    {
        app.view.transfer_target_notebook = first_ok;
    }
    let selected_title = app
        .session
        .notebooks
        .get(app.view.transfer_target_notebook)
        .map_or("Select notebook", |d| d.title.as_str())
        .to_string();
    crate::theme::studio_combo_box(
        ui,
        "transfer_target_notebook_combo",
        RichText::new(selected_title).size(12.0).color(t.text),
        ui.available_width().min(360.0),
        |ui| {
            for (idx, nb) in app.session.notebooks.iter().enumerate() {
                if Some(idx) == exclude_idx {
                    continue;
                }
                let label = format!("{} ({} pages)", nb.title, nb.page_count());
                ui.selectable_value(&mut app.view.transfer_target_notebook, idx, label);
            }
        },
    );
}

/// Renders the Transfer Pages or Merge Notebooks modal footer.
pub(crate) fn render_transfer_pages_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let mut should_close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let is_merge = app.view.transfer_merge_whole_notebook.is_some();
            let cta = if is_merge {
                "Merge Notebooks"
            } else if app.view.transfer_move_mode {
                "Move Pages"
            } else {
                "Copy Pages"
            };
            if primary_action_button(ui, cta, true, t).clicked() {
                if let Some(source_idx) = app.view.transfer_merge_whole_notebook.take() {
                    let _ = app.session.execute(
                        "notebook.merge",
                        &json!({
                            "source_index": source_idx,
                            "target_index": app.view.transfer_target_notebook,
                            "delete_source": app.view.transfer_merge_delete_source,
                        }),
                    );
                    crate::sync_view_to_active_notebook(app);
                } else {
                    let mut payload = json!({
                        "pages": app.view.transfer_pages_list,
                        "move_pages": app.view.transfer_move_mode,
                    });
                    if app.view.transfer_create_new_notebook {
                        payload["new_notebook_title"] = json!(app.view.transfer_new_title.trim());
                    } else {
                        payload["target_notebook"] = json!(app.view.transfer_target_notebook);
                    }
                    let _ = app.session.execute("page.transfer", &payload);
                    app.view.selected_page_indices.clear();
                    app.mesh_cache.clear();
                }
                should_close = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                app.view.transfer_merge_whole_notebook = None;
                should_close = true;
            }
        });
    });
    should_close
}

fn normalized_preview_title(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "Untitled Notebook"
    } else {
        trimmed
    }
}

fn draw_notebook_hero_card(
    ui: &mut egui::Ui,
    spec: &NotebookHeroSpec,
    title_buf: &mut String,
    t: &Tokens,
) {
    let preview_title = normalized_preview_title(title_buf).to_string();
    let avail_w = ui.available_width();
    let card_fill = t.toolbar_bg;
    egui::Frame::NONE
        .fill(card_fill)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(10.0)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if avail_w >= 460.0 {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    draw_live_pair_showcase(ui, spec, &preview_title, t);
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        draw_hero_title_column(ui, spec, title_buf, t);
                    });
                });
            } else {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    draw_hero_title_column(ui, spec, title_buf, t);
                    ui.add_space(10.0);
                    draw_live_pair_showcase(ui, spec, &preview_title, t);
                });
            }
        });
}

fn draw_live_pair_showcase(
    ui: &mut egui::Ui,
    spec: &NotebookHeroSpec,
    preview_title: &str,
    t: &Tokens,
) {
    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                let (book_rect, _) = ui.allocate_exact_size(Vec2::new(66.0, 90.0), Sense::hover());
                paint_hardcover_book_cover(
                    ui.painter(),
                    book_rect,
                    spec.cover,
                    Some(preview_title),
                    t.border,
                );

                let stage_size = Vec2::new(72.0, 90.0);
                let (stage_rect, _) = ui.allocate_exact_size(stage_size, Sense::hover());
                let (pw, ph) = spec.page_dimensions;
                let wf = pw.max(64) as f32;
                let hf = ph.max(64) as f32;
                let scale = (stage_size.x / wf).min(stage_size.y / hf);
                let sheet_size = Vec2::new((wf * scale).max(26.0), (hf * scale).max(26.0));
                let sheet_rect = Rect::from_center_size(stage_rect.center(), sheet_size);
                ui.painter().rect_filled(
                    sheet_rect.translate(Vec2::new(0.0, 2.0)),
                    4.0,
                    Color32::from_black_alpha(20),
                );
                let (tpl, p_color) = spec.first_page;
                let colors = p_color.resolved_colors(spec.theme, spec.auto_contrast);
                paint_paper_preview_colors(ui.painter(), sheet_rect, tpl, colors, t.border);
            });
        });
}

fn draw_hero_title_column(
    ui: &mut egui::Ui,
    spec: &NotebookHeroSpec,
    title_buf: &mut String,
    t: &Tokens,
) {
    ui.label(
        RichText::new("Notebook Name")
            .size(13.0)
            .strong()
            .color(t.text),
    );
    ui.label(
        RichText::new("Shown on your Library shelf, book label, and .dcraft files.")
            .size(11.0)
            .color(t.text_muted),
    );
    ui.add_space(6.0);

    draw_prominent_title_field(ui, spec.title_id_salt, spec.title_hint, title_buf, t);

    ui.add_space(7.0);
    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(6.0)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(
                    dot_rect.center(),
                    4.5,
                    to_color32(spec.cover.primary_color()),
                );
                ui.painter()
                    .circle_stroke(dot_rect.center(), 4.5, Stroke::new(0.8, t.border));
                ui.add(
                    egui::Label::new(RichText::new(&spec.summary).size(11.0).color(t.text_muted))
                        .truncate(),
                );
            });
        });
}

fn draw_prominent_title_field(
    ui: &mut egui::Ui,
    id_salt: &str,
    hint: &str,
    value: &mut String,
    t: &Tokens,
) {
    let edit_id = ui.make_persistent_id(id_salt);
    let has_focus = ui.ctx().memory(|m| m.has_focus(edit_id));
    let avail_w = ui.available_width().max(120.0);
    let box_fill = if t.is_dark {
        Color32::from_rgb(0x10, 0x13, 0x1A)
    } else {
        Color32::WHITE
    };
    let rest_border = if t.is_dark {
        t.text_muted.gamma_multiply(0.42)
    } else {
        t.text_muted.gamma_multiply(0.38)
    };
    let stroke = if has_focus {
        Stroke::new(1.75, t.accent)
    } else {
        Stroke::new(1.1, rest_border)
    };

    egui::Frame::NONE
        .fill(box_fill)
        .stroke(stroke)
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width((avail_w - 22.0).max(90.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                let icon_color = if has_focus { t.accent } else { t.text_muted };
                paint_icon(ui.painter(), icon_rect, Icon::Edit, icon_color);
                let input_w = (ui.available_width() - 4.0).max(70.0);
                ui.add(
                    egui::TextEdit::singleline(value)
                        .id(edit_id)
                        .desired_width(input_w)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::ZERO)
                        .align(egui::Align2::LEFT_CENTER)
                        .font(egui::FontId::proportional(15.0))
                        .text_color(t.text)
                        .hint_text(hint),
                );
            });
        });
}

fn draw_cover_shelf_and_custom_picker(
    ui: &mut egui::Ui,
    selected: &mut NotebookCover,
    draft: &mut CoverColorDraftState,
    t: &Tokens,
) {
    let avail_w = ui.available_width();
    let cols = if avail_w >= 540.0 {
        7
    } else if avail_w >= 340.0 {
        4
    } else {
        3
    };

    let mut items: Vec<Option<NotebookCover>> =
        NotebookCover::ALL.iter().copied().map(Some).collect();
    items.push(None); // Custom cover card.

    let rows: Vec<&[Option<NotebookCover>]> = items.chunks(cols).collect();
    let total_rows = rows.len();
    for (row_idx, row) in rows.into_iter().enumerate() {
        ui.columns(cols, |col_uis| {
            for (col_idx, item) in row.iter().enumerate() {
                let col_ui = &mut col_uis[col_idx];
                match item {
                    Some(cover) => draw_preset_cover_tile(col_ui, *cover, selected, draft, t),
                    None => draw_custom_cover_tile(col_ui, selected, draft, t),
                }
            }
        });
        if row_idx + 1 < total_rows {
            ui.add_space(6.0);
        }
    }

    let studio_id = ui.make_persistent_id("custom_cover_studio_collapse");
    crate::motion::animated_collapse(ui, studio_id, draft.open, 184.0, |ui| {
        ui.add_space(8.0);
        draw_inline_custom_cover_studio(ui, selected, draft, t);
    });
}

fn draw_preset_cover_tile(
    ui: &mut egui::Ui,
    cover: NotebookCover,
    selected: &mut NotebookCover,
    draft: &mut CoverColorDraftState,
    t: &Tokens,
) {
    let active = *selected == cover;
    let tile_id = ui.make_persistent_id(("cover_shelf_tile", cover.label()));
    let style = tile_motion_colors(ui, tile_id, active, t);

    let resp = egui::Frame::NONE
        .fill(style.fill)
        .stroke(style.stroke)
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(5, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                let (book_rect, _) = ui.allocate_exact_size(Vec2::new(34.0, 46.0), Sense::hover());
                paint_hardcover_book_cover(
                    ui.painter(),
                    book_rect,
                    cover,
                    None,
                    style.border_color,
                );
                ui.add_space(5.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(cover.label())
                            .size(10.5)
                            .strong()
                            .color(style.fg_color),
                    )
                    .wrap(),
                );
            });
        })
        .response;
    let click_resp = ui
        .interact(resp.rect, tile_id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(cover.description());

    ui.ctx()
        .data_mut(|d| d.insert_temp(tile_id.with("hov_mem"), click_resp.hovered()));
    if click_resp.clicked() {
        *selected = cover;
        draft.set_color(cover.primary_color());
        draft.open = false;
    }
}

fn draw_custom_cover_tile(
    ui: &mut egui::Ui,
    selected: &mut NotebookCover,
    draft: &mut CoverColorDraftState,
    t: &Tokens,
) {
    let is_custom = selected.is_custom();
    let preview_cover = if is_custom {
        *selected
    } else {
        draft.current_cover()
    };
    let tile_id = ui.make_persistent_id("cover_shelf_tile_custom");
    let style = tile_motion_colors(ui, tile_id, is_custom || draft.open, t);

    let resp = egui::Frame::NONE
        .fill(style.fill)
        .stroke(style.stroke)
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(5, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                let (book_rect, _) = ui.allocate_exact_size(Vec2::new(34.0, 46.0), Sense::hover());
                paint_hardcover_book_cover(
                    ui.painter(),
                    book_rect,
                    preview_cover,
                    None,
                    style.border_color,
                );

                // Subtle palette badge on the cover so its custom-color role is clear.
                let badge_c = pos2(book_rect.right() - 7.0, book_rect.bottom() - 7.0);
                ui.painter().circle_filled(badge_c, 7.5, t.field_bg);
                ui.painter()
                    .circle_stroke(badge_c, 7.5, Stroke::new(1.0, style.border_color));
                let badge_icon_rect = Rect::from_center_size(badge_c, Vec2::splat(9.5));
                paint_icon(ui.painter(), badge_icon_rect, Icon::Palette, t.accent);

                ui.add_space(5.0);
                let title = if is_custom { "Custom" } else { "Custom Color" };
                ui.add(
                    egui::Label::new(
                        RichText::new(title)
                            .size(10.5)
                            .strong()
                            .color(style.fg_color),
                    )
                    .wrap(),
                );
            });
        })
        .response;
    let tip = if is_custom {
        format!(
            "{} — Click to toggle color controls",
            preview_cover.display_label()
        )
    } else {
        "Pick any custom cover color using HSV sliders or a #RRGGBB hex code".to_string()
    };
    let click_resp = ui
        .interact(resp.rect, tile_id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip);

    ui.ctx()
        .data_mut(|d| d.insert_temp(tile_id.with("hov_mem"), click_resp.hovered()));
    if click_resp.clicked() {
        if !is_custom {
            *selected = draft.current_cover();
            draft.open = true;
        } else {
            draft.open = !draft.open;
        }
    }
}

fn draw_inline_custom_cover_studio(
    ui: &mut egui::Ui,
    selected: &mut NotebookCover,
    draft: &mut CoverColorDraftState,
    t: &Tokens,
) {
    let avail_w = ui.available_width();
    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(8.0)
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if avail_w >= 470.0 {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let mut changed =
                        paint_sv_square_picker(ui, Vec2::new(172.0, 76.0), &mut draft.hsv, t);
                    changed |=
                        paint_hue_bar_picker(ui, Vec2::new(16.0, 76.0), true, &mut draft.hsv[0], t);
                    if changed {
                        let c = draft.current_color();
                        draft.hex_draft = c.to_hex();
                        *selected = NotebookCover::from_color(c);
                    }
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        draw_custom_cover_hex_and_swatches(ui, selected, draft, t);
                    });
                });
            } else {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let sv_w = (ui.available_width() - 26.0).max(120.0);
                        let mut changed =
                            paint_sv_square_picker(ui, Vec2::new(sv_w, 72.0), &mut draft.hsv, t);
                        changed |= paint_hue_bar_picker(
                            ui,
                            Vec2::new(16.0, 72.0),
                            true,
                            &mut draft.hsv[0],
                            t,
                        );
                        if changed {
                            let c = draft.current_color();
                            draft.hex_draft = c.to_hex();
                            *selected = NotebookCover::from_color(c);
                        }
                    });
                    draw_custom_cover_hex_and_swatches(ui, selected, draft, t);
                });
            }
        });
}

fn draw_custom_cover_hex_and_swatches(
    ui: &mut egui::Ui,
    selected: &mut NotebookCover,
    draft: &mut CoverColorDraftState,
    t: &Tokens,
) {
    let current = draft.current_color();
    let [r, g, b, _] = current.to_rgba8();

    ui.horizontal(|ui| {
        ui.set_height(28.0);
        ui.spacing_mut().item_spacing.x = 8.0;
        let (swatch_rect, _) = ui.allocate_exact_size(Vec2::new(26.0, 24.0), Sense::hover());
        ui.painter()
            .rect_filled(swatch_rect, 5.0, to_color32(current));
        ui.painter().rect_stroke(
            swatch_rect,
            5.0,
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );

        crate::theme::studio_row_label(
            ui,
            RichText::new("Hex").size(11.5).color(t.text_muted),
            28.0,
        );
        let hex_resp = ui.add_sized(
            [78.0, 26.0],
            egui::TextEdit::singleline(&mut draft.hex_draft)
                .desired_width(74.0)
                .char_limit(7)
                .font(egui::TextStyle::Monospace)
                .margin(egui::Margin::symmetric(6, 2))
                .align(egui::Align2::LEFT_CENTER),
        );
        if hex_resp.changed()
            && let Some(parsed) = Color::from_hex(draft.hex_draft.trim())
        {
            draft.hsv = parsed.with_alpha(1.0).to_hsv();
            *selected = NotebookCover::from_color(parsed);
        }
        if hex_resp.lost_focus() {
            draft.hex_draft = draft.current_color().to_hex();
        }

        crate::theme::studio_row_label(
            ui,
            RichText::new(format!("{r}, {g}, {b}"))
                .size(10.5)
                .monospace()
                .color(t.text_muted),
            28.0,
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if compact_icon_button(ui, Icon::Check, Some("Done"), false, true, t)
                .on_hover_text("Collapse custom color controls while keeping this cover color")
                .clicked()
            {
                draft.open = false;
            }
        });
    });

    ui.add_space(7.0);
    ui.label(
        RichText::new("Quick cover colors")
            .size(10.5)
            .color(t.text_muted),
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(7.0, 6.0);
        for &(name, hex) in CUSTOM_COVER_QUICK_HEXES {
            let Some(swatch_color) = Color::from_hex(hex) else {
                continue;
            };
            let target_cover = NotebookCover::from_color(swatch_color);
            let active = *selected == target_cover;
            let (dot_rect, dot_resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
            if ui.is_rect_visible(dot_rect) {
                let p = ui.painter();
                if active {
                    p.circle_stroke(dot_rect.center(), 9.2, Stroke::new(1.8, t.accent));
                }
                p.circle_filled(dot_rect.center(), 7.2, to_color32(swatch_color));
                let rim = if t.is_dark {
                    Color32::from_white_alpha(48)
                } else {
                    Color32::from_black_alpha(38)
                };
                p.circle_stroke(dot_rect.center(), 7.2, Stroke::new(0.9, rim));
            }
            if dot_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("{name} ({hex})"))
                .clicked()
            {
                draft.set_color(swatch_color);
                *selected = target_cover;
            }
        }
    });
}

pub(crate) fn draw_custom_template_chip(
    ui: &mut egui::Ui,
    name: &str,
    active: bool,
    t: &Tokens,
) -> (bool, bool) {
    let pill_fill = if active { t.accent_soft } else { t.panel_bg };
    let pill_border = if active { t.accent } else { t.border };
    let mut select_clicked = false;
    let mut delete_clicked = false;
    egui::Frame::NONE
        .fill(pill_fill)
        .stroke(Stroke::new(1.0, pill_border))
        .corner_radius(7.0)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let name_color = if active { t.accent } else { t.text };
                if ui
                    .add(
                        egui::Label::new(RichText::new(name).size(11.5).strong().color(name_color))
                            .sense(Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    select_clicked = true;
                }
                if chip_close_button(ui, "Delete custom template", t.text_muted, t).clicked() {
                    delete_clicked = true;
                }
            });
        });
    (select_clicked, delete_clicked)
}

fn draw_paper_section_header_with_tint(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    selected_color: &mut PaperColor,
    t: &Tokens,
) {
    let avail_w = ui.available_width();
    if avail_w >= 540.0 {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).size(13.0).strong().color(t.text));
                ui.label(RichText::new(subtitle).size(11.0).color(t.text_muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                draw_sheet_tint_strip(ui, selected_color, t);
            });
        });
    } else {
        draw_modal_section_title(ui, title, subtitle, t);
        draw_sheet_tint_strip(ui, selected_color, t);
    }
}

#[derive(Clone, Copy)]
enum TemplatePickerTarget {
    NewNotebook,
    NotebookSettings,
}

fn draw_custom_templates_section(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    target: TemplatePickerTarget,
    t: &Tokens,
) {
    let apply_to_new_notebook = matches!(target, TemplatePickerTarget::NewNotebook);
    crate::theme::studio_wrapped_row(ui, Vec2::new(6.0, 6.0), 30.0, |ui| {
        crate::theme::studio_row_label(
            ui,
            RichText::new(if apply_to_new_notebook {
                "Your own background:"
            } else {
                "Custom Paper Templates:"
            })
            .size(11.5)
            .strong()
            .color(t.text),
            30.0,
        );
        let image_label = if apply_to_new_notebook {
            "Upload image..."
        } else {
            "Save Image as Template"
        };
        if secondary_action_button(ui, image_label, true, t)
            .on_hover_text(
                "Upload an image (PNG, JPG, WebP) to use as the page background; it is also saved for reuse",
            )
            .clicked()
        {
            app.trigger_import_custom_template(crate::PendingTemplateImport::Image {
                apply_to_page: false,
                apply_to_new_notebook,
            });
        }
        let pdf_label = if apply_to_new_notebook {
            "Upload PDF..."
        } else {
            "Save PDF as Template"
        };
        if secondary_action_button(ui, pdf_label, true, t)
            .on_hover_text("Use page 1 of a PDF as the page background; it is also saved for reuse")
            .clicked()
        {
            app.trigger_import_custom_template(crate::PendingTemplateImport::Pdf {
                apply_to_page: false,
                apply_to_new_notebook,
            });
        }
    });
    if let Some(ref err) = app.view.template_import_error {
        ui.label(RichText::new(err).size(11.0).color(t.danger_fg));
    }
    let custom_templates = app.session.prefs.custom_templates.clone();
    if custom_templates.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        let mut delete_id = None;
        for tpl in &custom_templates {
            let selected_id = match target {
                TemplatePickerTarget::NewNotebook => {
                    app.view.new_notebook_custom_template_id.as_deref()
                }
                TemplatePickerTarget::NotebookSettings => {
                    app.view.page_setup_custom_template_id.as_deref()
                }
            };
            let active = selected_id == Some(tpl.id.as_str());
            let (selected_clicked, del_clicked) =
                draw_custom_template_chip(ui, &tpl.name, active, t);
            if selected_clicked {
                match target {
                    TemplatePickerTarget::NewNotebook => {
                        app.view.new_notebook_custom_template_id = Some(tpl.id.clone());
                        app.view.new_notebook_format =
                            PageFormatDraft::from_dimensions(tpl.width, tpl.height);
                    }
                    TemplatePickerTarget::NotebookSettings => {
                        app.view.page_setup_custom_template_id = Some(tpl.id.clone());
                        app.view.page_setup_clear_custom = false;
                        app.view.page_setup_format =
                            PageFormatDraft::from_dimensions(tpl.width, tpl.height);
                        app.view.page_setup_quarter_turns = 0;
                    }
                }
            }
            if del_clicked {
                delete_id = Some(tpl.id.clone());
            }
        }
        if let Some(id) = delete_id {
            if app.view.new_notebook_custom_template_id.as_deref() == Some(id.as_str()) {
                app.view.new_notebook_custom_template_id = None;
            }
            if app.view.page_setup_custom_template_id.as_deref() == Some(id.as_str()) {
                app.view.page_setup_custom_template_id = None;
                app.view.page_setup_clear_custom =
                    app.session.doc.custom_template_id.as_deref() == Some(id.as_str());
            }
            let _ = app.session.execute("template.delete", &json!({ "id": id }));
        }
    });
}

#[allow(dead_code)]
fn draw_first_page_paper_section(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    draw_paper_section_header_with_tint(
        ui,
        "First Page Paper",
        "Choose the ruling sheet and paper tint for page 1.",
        &mut app.view.new_notebook_color,
        t,
    );

    ui.add_space(8.0);
    draw_paper_sheet_grid_for(
        ui,
        PaperGridTarget {
            id_salt: "new_nb_sheet_tile",
            theme: app.session.prefs.theme,
            auto_contrast: app.session.prefs.auto_contrast_with_theme,
            template: &mut app.view.new_notebook_template,
            color: app.view.new_notebook_color,
            custom_template_id: &mut app.view.new_notebook_custom_template_id,
            clear_custom: None,
        },
        t,
    );

    ui.add_space(10.0);
    draw_custom_templates_section(app, ui, TemplatePickerTarget::NewNotebook, t);
}

fn page_setup_effective_dims(app: &DrawCraftApp) -> (u32, u32) {
    app.view.page_setup_format.effective_dimensions()
}

fn page_setup_style_label(app: &DrawCraftApp) -> String {
    if let Some(ref tpl_id) = app.view.page_setup_custom_template_id
        && let Some(tpl) = app
            .session
            .prefs
            .custom_templates
            .iter()
            .find(|item| &item.id == tpl_id)
    {
        return tpl.name.clone();
    }
    app.view.page_setup_template.label().to_string()
}

fn draw_page_setup_sheet_preview(app: &DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (eff_w, eff_h) = page_setup_effective_dims(app);
    let turns = app.view.page_setup_quarter_turns.rem_euclid(4);
    let custom_tpl = app
        .view
        .page_setup_custom_template_id
        .as_ref()
        .and_then(|id| {
            app.session
                .prefs
                .custom_templates
                .iter()
                .find(|item| &item.id == id)
        });
    let has_doc_backdrop = !app.view.page_setup_clear_custom
        && custom_tpl.is_none()
        && (app.session.doc.pdf_backdrop.is_some()
            || app.session.doc.custom_image_backdrop.is_some());
    let colors = app.view.page_setup_color.resolved_colors(
        app.session.prefs.theme,
        app.session.prefs.auto_contrast_with_theme,
    );

    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(8.0)
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            let (stage_rect, _) = ui.allocate_exact_size(Vec2::new(72.0, 68.0), Sense::hover());
            let w_f = eff_w.max(1) as f32;
            let h_f = eff_h.max(1) as f32;
            let scale = (66.0 / w_f).min(64.0 / h_f);
            let sheet_size = Vec2::new((w_f * scale).max(28.0), (h_f * scale).max(28.0));
            let sheet_rect = Rect::from_center_size(stage_rect.center(), sheet_size);

            let p = ui.painter();
            p.rect_filled(
                sheet_rect.translate(Vec2::new(0.0, 2.0)),
                4.0,
                Color32::from_black_alpha(22),
            );

            let mut painted_backdrop = false;
            if let Some(tpl) = custom_tpl {
                p.rect_filled(sheet_rect, 4.0, to_color32(colors.background));
                painted_backdrop = crate::pdf_view::paint_custom_template_preview_rotated(
                    ui.ctx(),
                    p,
                    sheet_rect,
                    tpl,
                    240,
                    turns as u8,
                );
            } else if has_doc_backdrop {
                let fill = if app.session.doc.pdf_backdrop.is_some() {
                    Color32::WHITE
                } else {
                    to_color32(colors.background)
                };
                p.rect_filled(sheet_rect, 4.0, fill);
                painted_backdrop = crate::pdf_view::paint_doc_pdf_backdrop_with_rotation(
                    ui.ctx(),
                    p,
                    sheet_rect,
                    &app.session.doc,
                    240,
                    turns as u8,
                );
            }

            if painted_backdrop {
                p.rect_stroke(
                    sheet_rect,
                    4.0,
                    Stroke::new(1.0, t.border),
                    StrokeKind::Inside,
                );
            } else {
                let tpl = if custom_tpl.is_some() || has_doc_backdrop {
                    PaperTemplate::Blank
                } else {
                    app.view.page_setup_template
                };
                paint_paper_preview_colors_rotated(p, sheet_rect, tpl, colors, t.border, turns);
                if custom_tpl.is_some() || has_doc_backdrop {
                    let badge_label = if custom_tpl.is_some() {
                        "Custom"
                    } else {
                        "PDF"
                    };
                    let badge = Rect::from_center_size(sheet_rect.center(), Vec2::new(44.0, 16.0));
                    p.rect_filled(badge, 4.0, t.accent_soft);
                    p.rect_stroke(badge, 4.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
                    p.text(
                        badge.center(),
                        egui::Align2::CENTER_CENTER,
                        badge_label,
                        egui::FontId::proportional(9.5),
                        t.accent,
                    );
                }
            }

            if turns != 0 {
                let deg_label = format!("{}°", turns * 90);
                let pill = Rect::from_min_max(
                    pos2(stage_rect.right() - 28.0, stage_rect.bottom() - 15.0),
                    stage_rect.max,
                );
                p.rect_filled(pill, 4.0, t.accent_soft);
                p.rect_stroke(pill, 4.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
                p.text(
                    pill.center(),
                    egui::Align2::CENTER_CENTER,
                    deg_label,
                    egui::FontId::monospace(9.5),
                    t.accent,
                );
            }
        });
}

fn draw_page_setup_scope_strip(app: &DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let page_count = app.session.doc.page_count().max(1);
    let active_num = (app.session.doc.active_page_index() + 1).min(page_count);
    let (eff_w, eff_h) = page_setup_effective_dims(app);
    let turns = app.view.page_setup_quarter_turns.rem_euclid(4);

    egui::Frame::NONE
        .fill(t.toolbar_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            crate::theme::studio_wrapped_row(ui, Vec2::new(8.0, 4.0), 28.0, |ui| {
                crate::theme::studio_row_label(
                    ui,
                    RichText::new(format!("Page {active_num} of {page_count}"))
                        .size(12.5)
                        .strong()
                        .color(t.text),
                    28.0,
                );
                let dim_text = if turns == 0 {
                    format!("{eff_w} × {eff_h} pt")
                } else {
                    format!("{eff_w} × {eff_h} pt · {}°", turns * 90)
                };
                crate::theme::studio_row_label(
                    ui,
                    RichText::new(dim_text)
                        .size(11.0)
                        .monospace()
                        .color(if turns == 0 { t.text_muted } else { t.accent }),
                    28.0,
                );
            });
        });

    if app.session.doc.pdf_backdrop.is_some() {
        ui.add_space(6.0);
        egui::Frame::NONE
            .fill(t.accent_soft)
            .stroke(Stroke::new(1.0, t.accent))
            .corner_radius(6.0)
            .inner_margin(Margin::symmetric(8, 5))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "This page renders a PDF backdrop underneath your ink. Ruling and tint apply to non-PDF pages or new blank pages.",
                        )
                        .size(10.5)
                        .color(t.text),
                    )
                    .wrap(),
                );
            });
    }
}

fn draw_page_setup_paper_preview_banner(app: &DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (eff_w, eff_h) = page_setup_effective_dims(app);
    let turns = app.view.page_setup_quarter_turns.rem_euclid(4);
    let style_label = page_setup_style_label(app);
    let orient_label = app.view.page_setup_format.orientation.label();
    let colors = app.view.page_setup_color.resolved_colors(
        app.session.prefs.theme,
        app.session.prefs.auto_contrast_with_theme,
    );
    let summary_text = if turns == 0 {
        format!(
            "{style_label}  •  {}  •  {orient_label} ({eff_w} × {eff_h} pt)",
            app.view.page_setup_color.label(),
        )
    } else {
        format!(
            "{style_label}  •  {}  •  {orient_label} ({eff_w} × {eff_h} pt)  •  {}° CW",
            app.view.page_setup_color.label(),
            turns * 90,
        )
    };

    egui::Frame::NONE
        .fill(t.toolbar_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(9.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                draw_page_setup_sheet_preview(app, ui, t);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(style_label).size(13.0).strong().color(t.text));
                    ui.add_space(4.0);
                    egui::Frame::NONE
                        .fill(t.panel_bg)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(6.0)
                        .inner_margin(Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let (swatch_rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                                ui.painter().rect_filled(
                                    swatch_rect,
                                    2.5,
                                    to_color32(colors.background),
                                );
                                ui.painter().rect_stroke(
                                    swatch_rect,
                                    2.5,
                                    Stroke::new(0.8, t.border),
                                    StrokeKind::Inside,
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(summary_text).size(11.0).color(t.text_muted),
                                    )
                                    .wrap(),
                                );
                            });
                        });
                });
            });
        });
}

fn draw_sheet_tint_strip(ui: &mut egui::Ui, selected: &mut PaperColor, t: &Tokens) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for &color in PaperColor::ALL {
            let active = *selected == color;
            let pill_id = ui.scope_id().with(("sheet_tint_chip", color.label()));
            let style = tile_motion_colors(ui, pill_id, active, t);

            let resp = egui::Frame::NONE
                .fill(style.fill)
                .stroke(style.stroke)
                .corner_radius(7.0)
                .inner_margin(Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (swatch_rect, _) =
                            ui.allocate_exact_size(Vec2::new(18.0, 14.0), Sense::hover());
                        paint_tint_chip_swatch(
                            ui.painter(),
                            swatch_rect,
                            color,
                            style.border_color,
                        );
                        ui.label(
                            RichText::new(color.label())
                                .size(11.0)
                                .strong()
                                .color(style.fg_color),
                        );
                    });
                })
                .response;

            let click_resp = ui
                .interact(resp.rect, pill_id, Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(color.description());
            ui.ctx()
                .data_mut(|d| d.insert_temp(pill_id.with("hov_mem"), click_resp.hovered()));
            if click_resp.clicked() {
                *selected = color;
            }
        }
    });
}

fn paint_tint_chip_swatch(p: &egui::Painter, rect: Rect, color: PaperColor, border: Color32) {
    let bg = to_color32(color.background_color());
    let line = ensure_preview_line_contrast(bg, to_color32(color.line_color()));
    p.rect_filled(rect, 3.0, bg);
    let y1 = rect.top() + rect.height() * 0.38;
    let y2 = rect.top() + rect.height() * 0.68;
    p.line_segment(
        [pos2(rect.left() + 3.0, y1), pos2(rect.right() - 3.0, y1)],
        Stroke::new(0.9, line),
    );
    p.line_segment(
        [pos2(rect.left() + 3.0, y2), pos2(rect.right() - 3.0, y2)],
        Stroke::new(0.9, line),
    );
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, border), StrokeKind::Inside);
}

struct PaperGridTarget<'a> {
    id_salt: &'static str,
    theme: UiTheme,
    auto_contrast: bool,
    template: &'a mut PaperTemplate,
    color: PaperColor,
    custom_template_id: &'a mut Option<String>,
    clear_custom: Option<&'a mut bool>,
}

fn draw_paper_sheet_grid_for(ui: &mut egui::Ui, mut target: PaperGridTarget<'_>, t: &Tokens) {
    let cols = if ui.available_width() >= 540.0 { 6 } else { 3 };
    let rows: Vec<&[PaperTemplate]> = PaperTemplate::ALL.chunks(cols).collect();
    let total_rows = rows.len();
    let preview_colors = target
        .color
        .resolved_colors(target.theme, target.auto_contrast);

    for (row_idx, row) in rows.into_iter().enumerate() {
        ui.columns(cols, |col_uis| {
            for (col_idx, &template) in row.iter().enumerate() {
                let col_ui = &mut col_uis[col_idx];
                let selected = *target.template == template && target.custom_template_id.is_none();
                let tile_id = col_ui.make_persistent_id((target.id_salt, template.label()));
                let style = tile_motion_colors(col_ui, tile_id, selected, t);

                let resp = egui::Frame::NONE
                    .fill(style.fill)
                    .stroke(style.stroke)
                    .corner_radius(8.0)
                    .inner_margin(Margin::symmetric(5, 8))
                    .show(col_ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.vertical_centered(|ui| {
                            let (sheet_rect, _) =
                                ui.allocate_exact_size(Vec2::new(52.0, 68.0), Sense::hover());
                            ui.painter().rect_filled(
                                sheet_rect.translate(Vec2::new(0.0, 2.0)),
                                4.0,
                                Color32::from_black_alpha(20),
                            );
                            paint_paper_preview_colors(
                                ui.painter(),
                                sheet_rect,
                                template,
                                preview_colors,
                                style.border_color,
                            );
                            ui.add_space(6.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(template.label())
                                        .size(10.5)
                                        .strong()
                                        .color(style.fg_color),
                                )
                                .wrap(),
                            );
                        });
                    })
                    .response;
                let click_resp = col_ui
                    .interact(resp.rect, tile_id, Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(template.description());

                col_ui
                    .ctx()
                    .data_mut(|d| d.insert_temp(tile_id.with("hov_mem"), click_resp.hovered()));
                if click_resp.clicked() {
                    *target.template = template;
                    *target.custom_template_id = None;
                    if let Some(clear_flag) = target.clear_custom.as_deref_mut() {
                        *clear_flag = true;
                    }
                }
            }
        });
        if row_idx + 1 < total_rows {
            ui.add_space(6.0);
        }
    }
}

/// Names the notebook and explains its 30-day recovery period before it is confirmed.
pub(crate) fn render_delete_notebook_confirmation_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let target = app.view.pending_delete_notebook.as_ref();
    let target_exists = target.is_some_and(|target| {
        app.session
            .notebooks
            .iter()
            .any(|notebook| Arc::ptr_eq(notebook, target))
    });

    ui.vertical(|ui| {
        let heading = if target_exists {
            target.map_or_else(
                || "Move this notebook to Trash?".to_string(),
                |notebook| format!("Move \"{}\" to Trash?", notebook.title),
            )
        } else {
            "This notebook is no longer available".to_string()
        };
        ui.label(RichText::new(heading).size(15.0).strong().color(t.text));
        ui.add_space(8.0);
        let description = if target_exists {
            "Its pages and drawings will move to Trash for 30 days. Open Trash from the library to restore it before it expires."
        } else {
            "Close this dialog and choose an available notebook to move to Trash."
        };
        ui.label(RichText::new(description).size(12.0).color(t.text_muted));
    });
    false
}

/// Runs the engine Trash command only after the user confirms it.
pub(crate) fn render_delete_notebook_confirmation_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let pending_target = app.view.pending_delete_notebook.as_ref();
    let target_index = pending_target.and_then(|target| {
        app.session
            .notebooks
            .iter()
            .position(|notebook| Arc::ptr_eq(notebook, target))
    });
    let target_exists = target_index.is_some();
    let mut should_close = false;
    ui.horizontal(|ui| {
        if danger_action_button(ui, "Move to Trash", target_exists, t).clicked()
            && let Some(index) = target_index
        {
            app.flush_pending_autosaves();
            if app
                .session
                .execute("notebook.delete", &json!({ "index": index }))
                .is_ok()
            {
                crate::sync_view_to_active_notebook(app);
                if app.session.notebooks.is_empty() {
                    app.view.screen = crate::AppScreen::Home;
                }
                should_close = true;
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                should_close = true;
            }
        });
    });
    should_close
}

fn valid_pending_page_reorder(app: &DrawCraftApp) -> Option<crate::PendingPageReorder> {
    let pending = app.view.pending_page_reorder?;
    let page_count = app.session.doc.page_count();
    if pending.notebook_index == app.session.active_notebook
        && pending.from < page_count
        && pending.to < page_count
        && pending.from != pending.to
    {
        Some(pending)
    } else {
        None
    }
}

/// Shows the source and target page positions before reordering via drag-and-drop.
pub(crate) fn render_reorder_page_confirmation_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let pending = valid_pending_page_reorder(app);
    ui.vertical(|ui| {
        if let Some(req) = pending {
            let from_num = req.from + 1;
            let to_num = req.to + 1;
            let delta = req.from.abs_diff(req.to);
            let step_noun = if delta == 1 { "position" } else { "positions" };
            let direction = if req.to < req.from {
                format!("{delta} {step_noun} earlier")
            } else {
                format!("{delta} {step_noun} later")
            };

            egui::Frame::NONE
                .fill(t.panel_bg)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (badge, _) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::hover());
                        ui.painter()
                            .rect_filled(badge, CornerRadius::same(9), t.accent_soft);
                        paint_icon(ui.painter(), badge.shrink(9.0), Icon::Paper, t.accent);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(format!("Move Page {from_num} to position {to_num}?"))
                                    .size(14.5)
                                    .strong()
                                    .color(t.text),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "Page {from_num} -> Position {to_num}  •  {direction}"
                                ))
                                .size(11.5)
                                .color(t.text_muted),
                            );
                        });
                    });
                });
            ui.add_space(10.0);
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Pages in between will shift to keep your notebook in sequence. You can undo this reorder anytime.",
                    )
                    .size(12.0)
                    .color(t.text_muted),
                )
                .wrap(),
            );
        } else {
            ui.label(
                RichText::new("This page reorder is no longer available")
                    .size(15.0)
                    .strong()
                    .color(t.text),
            );
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    RichText::new("Close this dialog and try reordering the page again.")
                        .size(12.0)
                        .color(t.text_muted),
                )
                .wrap(),
            );
        }
    });
    false
}

/// Executes `page.move` only after the user confirms the dragged position change.
pub(crate) fn render_reorder_page_confirmation_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let valid_reorder = valid_pending_page_reorder(app);
    let target_valid = valid_reorder.is_some();
    let mut should_close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_action_button(ui, "Move Page", target_valid, t).clicked()
                && let Some(req) = valid_reorder
            {
                crate::canvas::commit_pending_text(app);
                if app
                    .session
                    .execute("page.move", &json!({ "from": req.from, "to": req.to }))
                    .is_ok()
                {
                    app.mesh_cache.clear();
                    should_close = true;
                }
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                should_close = true;
            }
        });
    });
    should_close
}

pub(crate) struct TileMotionColors {
    pub(crate) fill: Color32,
    pub(crate) border_color: Color32,
    pub(crate) stroke: Stroke,
    pub(crate) fg_color: Color32,
}

pub(crate) fn tile_motion_colors(
    ui: &egui::Ui,
    id: egui::Id,
    selected: bool,
    t: &Tokens,
) -> TileMotionColors {
    let prev_hovered = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(id.with("hov_mem")))
        .unwrap_or(false);
    let select_t = crate::motion::hover_factor(ui.ctx(), id.with("sel"), selected);
    let hover_t = crate::motion::hover_factor(ui.ctx(), id.with("hov"), prev_hovered && !selected);
    let rest_fill = crate::motion::lerp_color(t.panel_bg, t.border.gamma_multiply(0.3), hover_t);
    let fill = crate::motion::lerp_color(rest_fill, t.accent_soft, select_t);
    let rest_border =
        crate::motion::lerp_color(t.border, t.text_muted.gamma_multiply(0.55), hover_t);
    let border_color = crate::motion::lerp_color(rest_border, t.accent, select_t);
    let stroke = Stroke::new(crate::motion::lerp_f32(1.0, 1.5, select_t), border_color);
    let fg_color = crate::motion::lerp_color(t.text, t.accent, select_t);
    TileMotionColors {
        fill,
        border_color,
        stroke,
        fg_color,
    }
}

fn draw_modal_section_title(ui: &mut egui::Ui, title: &str, subtitle: &str, t: &Tokens) {
    ui.label(RichText::new(title).size(13.0).strong().color(t.text));
    ui.add(egui::Label::new(RichText::new(subtitle).size(11.0).color(t.text_muted)).wrap());
    ui.add_space(6.0);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn collect_shape_texts(shapes: &[egui::epaint::ClippedShape], out: &mut Vec<String>) {
        fn visit(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(items) => {
                    for s in items {
                        visit(s, out);
                    }
                }
                _ => {}
            }
        }
        for clipped in shapes {
            visit(&clipped.shape, out);
        }
    }

    #[test]
    fn new_notebook_and_settings_render_without_redundancy_and_support_custom_cover_color() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();

        modal::open(&mut app, modal::ActiveModal::NewNotebook);
        app.view.new_notebook_title = "Architecture Studio".to_string();
        app.view.new_notebook_cover = NotebookCover::custom_rgb(0x2B, 0x7A, 0x78);
        app.view
            .cover_color_draft
            .sync_from_cover(app.view.new_notebook_cover);
        assert!(app.view.cover_color_draft.open);
        assert_eq!(app.view.cover_color_draft.hex_draft, "#2B7A78");

        // Step 0 renders Notebook Name & Book Cover without Paper Ruling clutter.
        let mut new_nb_text = Vec::new();
        for _ in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_visuals(ui.ctx(), UiTheme::Dark);
                let t = Tokens::dark();
                render_new_notebook_body(&mut app, ui, &t);
                render_new_notebook_footer(&mut app, ui, &t);
            });
            new_nb_text.clear();
            collect_shape_texts(&out.shapes, &mut new_nb_text);
            out.drop_without_applying_deltas();
        }
        let joined = new_nb_text.join(" | ");
        // The dialog is deliberately flat: the title field carries its own text, no eyebrow labels.
        assert!(joined.contains("Architecture Studio"), "got: {joined}");
        assert!(!joined.contains("TITLE"), "got: {joined}");
        assert!(!joined.contains("PAPER SETTINGS"), "got: {joined}");
        assert!(joined.contains("COVER"), "got: {joined}");
        assert!(joined.contains("PAPER"), "got: {joined}");
        assert!(joined.contains("Paper Templates"), "got: {joined}");
        assert!(!joined.contains("Starter Preset"));

        // Switching to the Cover tab renders Cover Templates and custom cover colors cleanly.
        app.view.new_notebook_preview_tab = 1;
        let out_cover = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| {
                crate::theme::apply_visuals(ui.ctx(), UiTheme::Dark);
                let t = Tokens::dark();
                render_new_notebook_body(&mut app, ui, &t);
                render_new_notebook_footer(&mut app, ui, &t);
            },
        );
        let mut cover_text = Vec::new();
        collect_shape_texts(&out_cover.shapes, &mut cover_text);
        out_cover.drop_without_applying_deltas();
        let cover_joined = cover_text.join(" | ");
        assert!(
            cover_joined.contains("Cover Templates"),
            "got: {cover_joined}"
        );
        assert!(
            cover_joined.contains("Quick cover colors"),
            "got: {cover_joined}"
        );

        // Switching back to a preset collapses the inline custom studio cleanly.
        let mut preset_draft = CoverColorDraftState::default();
        preset_draft.sync_from_cover(NotebookCover::SageGreen);
        assert!(!preset_draft.open);

        // Notebook settings also renders all 3 unified tabs with custom cover studio open on Tab 0.
        app.session
            .execute(
                "notebook.create",
                &json!({
                    "title": "Field Sketchbook",
                    "cover": NotebookCover::custom_rgb(0xBE, 0x12, 0x3C),
                    "template": PaperTemplate::Dotted,
                    "paper_color": PaperColor::WarmCream,
                }),
            )
            .unwrap();
        modal::open(&mut app, modal::ActiveModal::NotebookSettings);
        assert!(app.view.cover_color_draft.open);
        assert_eq!(app.view.cover_color_draft.hex_draft, "#BE123C");

        for step in [0usize, 1usize, 2usize] {
            app.view.notebook_settings_step = step;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                let t = Tokens::light();
                render_notebook_settings_body(&mut app, ui, &t);
                render_notebook_settings_footer(&mut app, ui, &t);
            });
            assert!(!out.shapes.is_empty());
            out.drop_without_applying_deltas();
        }
    }

    fn find_modal_label_center(
        shapes: &[egui::epaint::ClippedShape],
        needle: &str,
    ) -> Option<egui::Pos2> {
        fn visit(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == needle => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                egui::Shape::Vec(items) => items.iter().find_map(|s| visit(s, needle)),
                _ => None,
            }
        }
        shapes.iter().find_map(|c| visit(&c.shape, needle))
    }

    fn run_page_setup_modal_frame(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        events: Vec<egui::Event>,
        time: f64,
    ) -> Vec<egui::epaint::ClippedShape> {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1280.0, 800.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut shapes = Vec::new();
        let out = ctx.run_ui(raw, |ui| {
            crate::theme::apply_light_visuals(ui.ctx());
            modal::show(app, ui);
        });
        shapes.extend(out.shapes.clone());
        out.drop_without_applying_deltas();
        shapes
    }

    fn click_page_setup_label(ctx: &egui::Context, app: &mut DrawCraftApp, label: &str, t0: f64) {
        let shapes = run_page_setup_modal_frame(ctx, app, Vec::new(), t0);
        let pos = find_modal_label_center(&shapes, label)
            .unwrap_or_else(|| panic!("expected modal label {label:?}"));
        for (pressed, dt) in [(true, 0.02), (false, 0.04)] {
            let _ = run_page_setup_modal_frame(
                ctx,
                app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                t0 + dt,
            );
        }
        let _ = run_page_setup_modal_frame(ctx, app, Vec::new(), t0 + 0.08);
    }

    fn settle_modal_frames(ctx: &egui::Context, app: &mut DrawCraftApp, mut t: f64) {
        for _ in 0..8 {
            let _ = run_page_setup_modal_frame(ctx, app, Vec::new(), t);
            t += 0.05;
        }
    }

    #[test]
    fn page_setup_modal_supports_live_preview_cancel_apply_all_and_add_page() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));
        let orig_tpl = app.session.doc.paper_template;
        let orig_color = app.session.doc.paper_color;
        let (orig_w, orig_h) = app.session.doc.page_dimensions(0).unwrap();

        // Opening NotebookSettings starts on Tab 0 (Details & Cover); switch tabs via header pills; Cancel leaves document unchanged.
        modal::open(&mut app, modal::ActiveModal::NotebookSettings);
        assert_eq!(app.view.notebook_settings_step, 0);
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.05);
        click_page_setup_label(&ctx, &mut app, "Size & Orientation", 0.12);
        assert_eq!(app.view.notebook_settings_step, 1);
        click_page_setup_label(&ctx, &mut app, "A4", 0.24);
        click_page_setup_label(&ctx, &mut app, "Landscape", 0.36);
        assert_eq!(
            app.view.page_setup_format.preset,
            drawcraft_doc::PageResolutionPreset::A4
        );
        assert_eq!(
            app.view.page_setup_format.orientation,
            drawcraft_doc::PageOrientation::Landscape
        );
        click_page_setup_label(&ctx, &mut app, "Paper & Templates", 0.48);
        assert_eq!(app.view.notebook_settings_step, 2);

        click_page_setup_label(&ctx, &mut app, PaperTemplate::Grid.label(), 0.60);
        click_page_setup_label(&ctx, &mut app, PaperColor::DarkSlate.label(), 0.72);
        assert_eq!(app.view.page_setup_template, PaperTemplate::Grid);
        assert_eq!(app.view.page_setup_color, PaperColor::DarkSlate);
        assert_eq!(app.session.doc.paper_template, orig_tpl);

        click_page_setup_label(&ctx, &mut app, "Cancel", 0.84);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.session.doc.paper_template, orig_tpl);
        assert_eq!(app.session.doc.paper_color, orig_color);
        assert_eq!(
            app.session.doc.page_dimensions(0).unwrap(),
            (orig_w, orig_h)
        );

        // Re-open, switch to Size & Orientation (A5 Landscape + All 2 Pages), then Paper & Templates -> Dotted + WarmCream -> Apply.
        settle_modal_frames(&ctx, &mut app, 1.0);
        modal::open(&mut app, modal::ActiveModal::NotebookSettings);
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 1.5);
        click_page_setup_label(&ctx, &mut app, "Size & Orientation", 1.58);
        click_page_setup_label(&ctx, &mut app, "A5 Compact", 1.70);
        click_page_setup_label(&ctx, &mut app, "Landscape", 1.82);
        click_page_setup_label(&ctx, &mut app, "All 2 Pages", 1.94);
        click_page_setup_label(&ctx, &mut app, "Paper & Templates", 2.06);
        click_page_setup_label(&ctx, &mut app, PaperTemplate::Dotted.label(), 2.20);
        click_page_setup_label(&ctx, &mut app, PaperColor::WarmCream.label(), 2.35);
        click_page_setup_label(&ctx, &mut app, "Apply to All Pages", 2.50);

        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.session.doc.paper_template, PaperTemplate::Dotted);
        assert_eq!(app.session.doc.paper_color, PaperColor::WarmCream);
        assert_eq!(app.session.doc.page_dimensions(0).unwrap(), (1190, 840));
        assert_eq!(app.session.doc.page_dimensions(1).unwrap(), (1190, 840));

        // Re-open, switch active page to US Letter Portrait on Tab 1, switch to Tab 2 -> Checklist -> + Add Page.
        settle_modal_frames(&ctx, &mut app, 2.7);
        modal::open(&mut app, modal::ActiveModal::NotebookSettings);
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 3.2);
        click_page_setup_label(&ctx, &mut app, "Size & Orientation", 3.28);
        click_page_setup_label(&ctx, &mut app, "US Letter", 3.40);
        click_page_setup_label(&ctx, &mut app, "Portrait", 3.52);
        click_page_setup_label(&ctx, &mut app, "Paper & Templates", 3.64);
        click_page_setup_label(&ctx, &mut app, PaperTemplate::Checklist.label(), 3.76);
        click_page_setup_label(&ctx, &mut app, "+ Add Page with Style", 3.90);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.session.doc.page_count(), 3);
        assert_eq!(app.session.doc.paper_template, PaperTemplate::Checklist);
        assert_eq!(app.session.doc.width, 1224);
        assert_eq!(app.session.doc.height, 1584);
        // Existing pages stayed at A5 Landscape independently of the newly inserted page.
        assert_eq!(app.session.doc.page_dimensions(0).unwrap(), (1190, 840));
    }

    fn find_modal_text_center_y(
        shapes: &[egui::epaint::ClippedShape],
        needle: &str,
    ) -> Option<f32> {
        fn visit(shape: &egui::epaint::Shape, needle: &str) -> Option<f32> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == needle => {
                    Some(text.pos.y + text.galley.size().y * 0.5)
                }
                egui::epaint::Shape::Vec(children) => {
                    children.iter().find_map(|child| visit(child, needle))
                }
                _ => None,
            }
        }
        shapes.iter().find_map(|c| visit(&c.shape, needle))
    }

    #[test]
    fn page_setup_modal_wraps_and_aligns_inline_row_labels() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));
        modal::open(&mut app, modal::ActiveModal::NotebookSettings);
        app.view.notebook_settings_step = 1;

        let mut shapes = Vec::new();
        for frame in 0..8 {
            shapes =
                run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), f64::from(frame) * 0.05);
        }

        let pairs_step1 = [
            ("Orientation:", "Portrait"),
            ("Apply to:", "This Page Only"),
        ];
        for (label, control) in pairs_step1 {
            let ly = find_modal_text_center_y(&shapes, label)
                .unwrap_or_else(|| panic!("expected label {label:?}"));
            let cy = find_modal_text_center_y(&shapes, control)
                .unwrap_or_else(|| panic!("expected control {control:?}"));
            assert!(
                (ly - cy).abs() <= 1.5,
                "{label:?} ({ly:.2}) and {control:?} ({cy:.2}) should align vertically"
            );
        }

        app.view.notebook_settings_step = 2;
        let mut shapes_step2 = Vec::new();
        for frame in 0..5 {
            shapes_step2 = run_page_setup_modal_frame(
                &ctx,
                &mut app,
                Vec::new(),
                0.5 + f64::from(frame) * 0.05,
            );
        }
        let ly = find_modal_text_center_y(&shapes_step2, "Custom Paper Templates:")
            .unwrap_or_else(|| panic!("expected Custom Paper Templates label"));
        let cy = find_modal_text_center_y(&shapes_step2, "Save Image as Template")
            .unwrap_or_else(|| panic!("expected Save Image as Template control"));
        assert!(
            (ly - cy).abs() <= 1.5,
            "Custom Paper Templates ({ly:.2}) and Save Image as Template ({cy:.2}) should align vertically"
        );

        // Verify custom cover Hex + input + RGB + Done row shares one horizontal centerline.
        let t = crate::theme::Tokens::light();
        let mut selected = NotebookCover::default();
        let mut draft = CoverColorDraftState {
            open: true,
            hsv: Color::from_hex("#2563EB")
                .unwrap_or(Color::BLACK)
                .with_alpha(1.0)
                .to_hsv(),
            hex_draft: "#2563EB".to_string(),
        };
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(600.0, 400.0),
            )),
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| {
            crate::theme::apply_light_visuals(ui.ctx());
            draw_custom_cover_hex_and_swatches(ui, &mut selected, &mut draft, &t);
        });
        let row_shapes = out.shapes.clone();
        out.drop_without_applying_deltas();

        let ys: Vec<f32> = ["Hex", "#2563EB", "37, 99, 235", "Done"]
            .iter()
            .map(|needle| {
                find_modal_text_center_y(&row_shapes, needle)
                    .unwrap_or_else(|| panic!("expected custom cover row text {needle:?}"))
            })
            .collect();
        let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
        let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max_y - min_y <= 1.5,
            "custom cover Hex/RGB/Done row should share vertical center: {ys:?}"
        );
    }

    fn collect_line_segments(shapes: &[egui::epaint::ClippedShape]) -> Vec<[egui::Pos2; 2]> {
        let mut out = Vec::new();
        fn visit(shape: &egui::epaint::Shape, out: &mut Vec<[egui::Pos2; 2]>) {
            match shape {
                egui::epaint::Shape::LineSegment { points, .. } => out.push(*points),
                egui::epaint::Shape::Vec(children) => {
                    for child in children {
                        visit(child, out);
                    }
                }
                _ => {}
            }
        }
        for clipped in shapes {
            visit(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn page_setup_footer_places_scope_switch_beside_save_without_overlap() {
        fn text_rect(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Option<Rect> {
            fn visit(shape: &egui::epaint::Shape, needle: &str) -> Option<Rect> {
                match shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == needle => {
                        Some(Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    egui::epaint::Shape::Vec(children) => {
                        children.iter().find_map(|child| visit(child, needle))
                    }
                    _ => None,
                }
            }
            shapes.iter().find_map(|c| visit(&c.shape, needle))
        }

        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        for _ in 0..11 {
            let _ = app.session.execute("page.add", &json!({}));
        }
        modal::open(&mut app, modal::ActiveModal::NotebookSettings);

        for step in [1usize, 2] {
            app.view.notebook_settings_step = step;
            app.view.page_setup_apply_all = true;
            let mut shapes = Vec::new();
            for frame in 0..8 {
                shapes = run_page_setup_modal_frame(
                    &ctx,
                    &mut app,
                    Vec::new(),
                    step as f64 + f64::from(frame) * 0.05,
                );
            }
            let add = text_rect(&shapes, "+ Add Page with Style").unwrap_or(Rect::NOTHING);
            let label = text_rect(&shapes, "Apply to:").unwrap_or(Rect::NOTHING);
            let single = text_rect(&shapes, "This Page Only").unwrap_or(Rect::NOTHING);
            let all = text_rect(&shapes, "All 12 Pages").unwrap_or(Rect::NOTHING);
            let cancel = text_rect(&shapes, "Cancel").unwrap_or(Rect::NOTHING);
            let save = text_rect(&shapes, "Apply to All Pages").unwrap_or(Rect::NOTHING);
            for r in [add, label, single, all, cancel, save] {
                assert!(r.is_positive(), "missing footer text on step {step}");
            }
            assert!((label.center().y - save.center().y).abs() <= 1.5);
            assert!(add.right() < label.left(), "{add:?} overlaps {label:?}");
            assert!(label.right() < single.left());
            assert!(single.right() < all.left());
            assert!(all.right() < cancel.left());
            assert!(cancel.right() < save.left());
        }
        assert!(
            text_rect(
                &run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 3.0),
                "Rotate 90°"
            )
            .is_none(),
            "Rotate lives in the orientation row, not on the paper tab"
        );
    }

    #[test]
    fn page_setup_rotate_90_rotates_preview_across_all_four_angles() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        modal::open(&mut app, modal::ActiveModal::NotebookSettings);
        app.view.notebook_settings_step = 1;
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.05);
        assert_eq!(app.view.page_setup_quarter_turns, 0);

        for (step, expected_deg) in [(1, "90°"), (2, "180°"), (3, "270°")] {
            click_page_setup_label(&ctx, &mut app, "Rotate 90°", 0.2 * f64::from(step));
            assert_eq!(app.view.page_setup_quarter_turns, step);
            let shapes =
                run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.2 * f64::from(step) + 0.1);
            let mut texts = Vec::new();
            collect_shape_texts(&shapes, &mut texts);
            assert!(
                texts.iter().any(|t| t == expected_deg),
                "expected rotation badge {expected_deg:?} in {texts:?}"
            );
        }

        click_page_setup_label(&ctx, &mut app, "Rotate 90°", 0.9);
        assert_eq!(app.view.page_setup_quarter_turns, 0);

        // Verify RuledNarrow red margin line rotates left -> top -> right -> bottom across 0..4 turns.
        let colors = PaperColor::WarmCream.resolved_colors(UiTheme::Light, false);
        let mut margin_positions = Vec::new();
        for turns in 0..4 {
            let rect = if turns % 2 == 1 {
                Rect::from_center_size(pos2(100.0, 100.0), Vec2::new(76.0, 57.0))
            } else {
                Rect::from_center_size(pos2(100.0, 100.0), Vec2::new(57.0, 76.0))
            };
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(200.0, 200.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                paint_paper_preview_colors_rotated(
                    ui.painter(),
                    rect,
                    PaperTemplate::RuledNarrow,
                    colors,
                    Color32::GRAY,
                    turns,
                );
            });
            let segs = collect_line_segments(&out.shapes);
            out.drop_without_applying_deltas();
            // Last segment emitted by RuledNarrow is the margin line.
            margin_positions.push(*segs.last().expect("margin segment"));
        }
        // 0 deg: vertical on left half (< 100).
        assert!((margin_positions[0][0].x - margin_positions[0][1].x).abs() < 0.1);
        assert!(margin_positions[0][0].x < 100.0);
        // 90 deg: horizontal on top half (< 100).
        assert!((margin_positions[1][0].y - margin_positions[1][1].y).abs() < 0.1);
        assert!(margin_positions[1][0].y < 100.0);
        // 180 deg: vertical on right half (> 100).
        assert!((margin_positions[2][0].x - margin_positions[2][1].x).abs() < 0.1);
        assert!(margin_positions[2][0].x > 100.0);
        // 270 deg: horizontal on bottom half (> 100).
        assert!((margin_positions[3][0].y - margin_positions[3][1].y).abs() < 0.1);
        assert!(margin_positions[3][0].y > 100.0);
    }

    #[test]
    fn new_notebook_modal_supports_resolution_presets_and_custom_dimensions() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();

        modal::open(&mut app, modal::ActiveModal::NewNotebook);
        assert_eq!(app.view.new_notebook_preview_tab, 0);
        assert_eq!(
            app.view.new_notebook_format.preset,
            drawcraft_doc::PageResolutionPreset::Standard
        );

        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.02);
        // Toggle orientation via the single-screen Portrait button and pick A4 preset.
        app.view.new_notebook_format.preset = drawcraft_doc::PageResolutionPreset::A4;
        click_page_setup_label(&ctx, &mut app, "Portrait", 0.12);
        assert_eq!(
            app.view.new_notebook_format.orientation,
            drawcraft_doc::PageOrientation::Landscape
        );
        app.view.new_notebook_title = "Deck A4".to_string();
        click_page_setup_label(&ctx, &mut app, "Create Notebook", 0.24);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.session.doc.title, "Deck A4");
        assert_eq!(app.session.doc.page_dimensions(0), Some((1684, 1190)));

        // Re-open, configure custom dimensions, and create directly on one screen.
        settle_modal_frames(&ctx, &mut app, 0.40);
        modal::open(&mut app, modal::ActiveModal::NewNotebook);
        app.view.new_notebook_title = "UltraWide".to_string();
        app.view
            .new_notebook_format
            .set_custom_dimensions(2560, 1440);
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.90);
        click_page_setup_label(&ctx, &mut app, "Create Notebook", 1.00);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.session.doc.title, "UltraWide");
        assert_eq!(app.session.doc.page_dimensions(0), Some((2560, 1440)));
    }

    #[test]
    fn new_notebook_creates_notebook_with_favorite_and_tags() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "library.folder.create",
                &json!({ "path": "College/Spring" }),
            )
            .unwrap();
        // Navigate into "College/Spring" — New Notebook must create directly in the current folder.
        crate::library_folders::set_scope(
            &mut app,
            crate::library_folders::LibraryScope::Folder("College/Spring".to_string()),
        );

        modal::open(&mut app, modal::ActiveModal::NewNotebook);
        assert_eq!(app.view.new_notebook_folder, "College/Spring");
        app.view.new_notebook_title = "Organic Chemistry".to_string();
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.05);
        click_page_setup_label(&ctx, &mut app, "Create Notebook", 0.16);

        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.session.doc.title, "Organic Chemistry");
        assert_eq!(app.session.doc.folder_path, "College/Spring");

        // Opening New Notebook from Favorites auto-stars the newly created notebook.
        settle_modal_frames(&ctx, &mut app, 0.30);
        crate::library_folders::set_scope(
            &mut app,
            crate::library_folders::LibraryScope::Favorites,
        );
        modal::open(&mut app, modal::ActiveModal::NewNotebook);
        app.view.new_notebook_title = "Starred Notes".to_string();
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.80);
        click_page_setup_label(&ctx, &mut app, "Create Notebook", 0.92);
        assert_eq!(app.session.doc.title, "Starred Notes");
        assert!(app.session.doc.favorite);
    }

    #[test]
    fn notebook_settings_edits_inline_tags_favorite_cover_and_page_style_end_to_end() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "notebook.setMetadata",
                &json!({
                    "index": 0,
                    "folder_path": "School/Term1",
                    "favorite": false,
                    "tags": ["draft", "physics"],
                }),
            )
            .unwrap();

        modal::open(&mut app, modal::ActiveModal::NotebookSettings);
        let _ = run_page_setup_modal_frame(&ctx, &mut app, Vec::new(), 0.05);

        // Click #draft chip text -> enters inline tag edit mode -> rename -> Save Changes commits it.
        click_page_setup_label(&ctx, &mut app, "#draft", 0.15);
        assert_eq!(app.view.tag_edit_index, Some(0));
        assert_eq!(app.view.tag_edit_draft, "draft");
        app.view.tag_edit_draft = "final".to_string();
        app.view.notebook_title_draft = "Physics Honors".to_string();
        app.view.notebook_tag_input_draft = "2026".to_string();

        click_page_setup_label(&ctx, &mut app, "Mark Favorite", 0.25);
        click_page_setup_label(&ctx, &mut app, "Sage Green", 0.35);
        click_page_setup_label(&ctx, &mut app, "Paper & Templates", 0.45);
        click_page_setup_label(&ctx, &mut app, PaperTemplate::Grid.label(), 0.55);
        click_page_setup_label(&ctx, &mut app, "Save Changes", 0.65);

        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.session.doc.title, "Physics Honors");
        assert_eq!(app.session.doc.cover, NotebookCover::SageGreen);
        assert!(app.session.doc.favorite);
        assert_eq!(app.session.doc.tags, vec!["final", "physics", "2026"]);
        assert_eq!(app.session.doc.paper_template, PaperTemplate::Grid);
    }
}
