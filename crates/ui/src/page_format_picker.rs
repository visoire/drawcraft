//! Reusable Page Size & Orientation picker component shared by New Notebook and Page Setup dialogs.

use drawcraft_doc::{PageOrientation, PageResolutionPreset, PaperTemplate, ResolvedPaperColors};
use egui::{CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::customization_modals::{paint_paper_preview_colors_rotated, tile_motion_colors};
use crate::icons::{
    Icon, SwitchLayout, SwitchOption, compact_icon_button, section_card_header, segmented_switch,
};
use crate::inspector::card_frame;
use crate::theme::{Tokens, to_color32};

/// Draft state for choosing a page size preset, orientation, and custom dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFormatDraft {
    pub preset: PageResolutionPreset,
    pub orientation: PageOrientation,
    pub custom_width: u32,
    pub custom_height: u32,
}

impl Default for PageFormatDraft {
    fn default() -> Self {
        Self::default_standard()
    }
}

impl PageFormatDraft {
    /// Initializes draft state with the studio's default `Standard` Portrait sheet (`1200 × 1600`).
    #[must_use]
    pub const fn default_standard() -> Self {
        Self {
            preset: PageResolutionPreset::Standard,
            orientation: PageOrientation::Portrait,
            custom_width: 1200,
            custom_height: 1600,
        }
    }

    /// Initializes draft state from concrete sheet `(width, height)` dimensions.
    #[must_use]
    pub fn from_dimensions(width: u32, height: u32) -> Self {
        let w = width.clamp(64, 8192);
        let h = height.clamp(64, 8192);
        let (preset, orientation) = PageResolutionPreset::detect(w, h);
        Self {
            preset,
            orientation,
            custom_width: w,
            custom_height: h,
        }
    }

    /// Returns the resolved `(width, height)` dimensions clamped to valid document bounds.
    #[must_use]
    pub const fn effective_dimensions(self) -> (u32, u32) {
        const fn clamp_dim(v: u32) -> u32 {
            if v < 64 {
                64
            } else if v > 8192 {
                8192
            } else {
                v
            }
        }
        if matches!(self.preset, PageResolutionPreset::Custom) {
            (clamp_dim(self.custom_width), clamp_dim(self.custom_height))
        } else {
            self.preset.dimensions_with_orientation(self.orientation)
        }
    }

    /// Selects a resolution preset card while preserving consistent orientation semantics.
    pub fn select_preset(&mut self, next_preset: PageResolutionPreset) {
        if next_preset == PageResolutionPreset::Custom {
            if self.preset != PageResolutionPreset::Custom {
                let (w, h) = self.effective_dimensions();
                self.custom_width = w;
                self.custom_height = h;
            }
            self.preset = PageResolutionPreset::Custom;
            self.orientation =
                PageOrientation::from_dimensions(self.custom_width, self.custom_height);
            return;
        }
        self.preset = next_preset;
        let (w, h) = self.effective_dimensions();
        self.custom_width = w;
        self.custom_height = h;
    }

    /// Applies an orientation (`Portrait` or `Landscape`) to the draft, swapping axes when needed.
    pub fn set_orientation(&mut self, orientation: PageOrientation) {
        self.orientation = orientation;
        let (cw, ch) = orientation.apply(
            self.custom_width.clamp(64, 8192),
            self.custom_height.clamp(64, 8192),
        );
        self.custom_width = cw;
        self.custom_height = ch;
    }

    /// Updates custom width/height explicitly while keeping `Custom` selected for live typing/drags.
    pub fn set_custom_dimensions(&mut self, width: u32, height: u32) {
        let w = width.clamp(64, 8192);
        let h = height.clamp(64, 8192);
        self.preset = PageResolutionPreset::Custom;
        self.custom_width = w;
        self.custom_height = h;
        self.orientation = PageOrientation::from_dimensions(w, h);
    }

    /// Swaps width and height (`Rotate 90°` equivalent for sheet format proportions).
    pub fn swap_dimensions(&mut self) {
        self.orientation = self.orientation.toggled();
        let (w, h) = (
            self.custom_height.clamp(64, 8192),
            self.custom_width.clamp(64, 8192),
        );
        self.custom_width = w;
        self.custom_height = h;
    }
}

/// Visual presentation context passed from the hosting dialog into [`render_page_format_picker`].
pub struct PageFormatPreviewContext<'a> {
    pub id_prefix: &'a str,
    pub paper_colors: ResolvedPaperColors,
    pub paper_template: PaperTemplate,
    pub preview_turns: i32,
    pub custom_backdrop_active: bool,
    /// When present, a `Rotate 90°` button is shown next to the orientation switch; clicking it
    /// advances these quarter turns and swaps the draft's width and height.
    pub rotate_turns: Option<&'a mut i32>,
}

fn preset_display_title(
    preset: PageResolutionPreset,
    orientation: PageOrientation,
) -> &'static str {
    if preset == PageResolutionPreset::Widescreen && orientation == PageOrientation::Portrait {
        "9:16 Tall"
    } else {
        preset.display_name()
    }
}

fn preset_ratio_badge(preset: PageResolutionPreset, orientation: PageOrientation) -> &'static str {
    if preset == PageResolutionPreset::Widescreen && orientation == PageOrientation::Portrait {
        "9:16"
    } else {
        preset.ratio_badge()
    }
}

fn fit_proportional_rect(container: Rect, width: u32, height: u32) -> Rect {
    let w = (width.max(64)) as f32;
    let h = (height.max(64)) as f32;
    let scale = (container.width() / w).min(container.height() / h);
    let rw = (w * scale).max(12.0).min(container.width());
    let rh = (h * scale).max(12.0).min(container.height());
    Rect::from_center_size(container.center(), vec2(rw, rh))
}

fn paint_badge_pill(
    ui: &mut egui::Ui,
    text: &str,
    fill: egui::Color32,
    stroke: egui::Color32,
    text_color: egui::Color32,
) {
    egui::Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(10.5)
                    .strong()
                    .color(text_color),
            );
        });
}

fn paint_preset_mini_sheet(
    painter: &egui::Painter,
    bounds: Rect,
    preset: PageResolutionPreset,
    orientation: PageOrientation,
    paper_colors: ResolvedPaperColors,
    selected: bool,
    t: &Tokens,
) {
    let (w, h) = if preset == PageResolutionPreset::Custom {
        orientation.apply(1200, 1600)
    } else {
        preset.dimensions_with_orientation(orientation)
    };
    let sheet_rect = fit_proportional_rect(bounds.shrink2(vec2(6.0, 4.0)), w, h);
    let shadow_rect = sheet_rect.translate(vec2(1.5, 2.0));
    painter.rect_filled(
        shadow_rect,
        CornerRadius::same(3),
        egui::Color32::from_black_alpha(if t.is_dark { 70 } else { 22 }),
    );
    let bg = if preset == PageResolutionPreset::Custom {
        t.field_bg
    } else {
        to_color32(paper_colors.background)
    };
    painter.rect_filled(sheet_rect, CornerRadius::same(3), bg);
    let border_col = if selected { t.accent } else { t.border };
    if preset == PageResolutionPreset::Custom {
        let stroke = Stroke::new(1.3, border_col);
        let dash = 4.0_f32;
        let gap = 3.0_f32;
        let corners = [
            (sheet_rect.left_top(), sheet_rect.right_top()),
            (sheet_rect.right_top(), sheet_rect.right_bottom()),
            (sheet_rect.right_bottom(), sheet_rect.left_bottom()),
            (sheet_rect.left_bottom(), sheet_rect.left_top()),
        ];
        for (a, b) in corners {
            let delta = b - a;
            let len = delta.length();
            if len > 0.5 {
                let dir = delta / len;
                let mut pos = 0.0_f32;
                while pos < len {
                    let end = (pos + dash).min(len);
                    painter.line_segment([a + dir * pos, a + dir * end], stroke);
                    pos += dash + gap;
                }
            }
        }
    } else {
        painter.rect_stroke(
            sheet_rect,
            CornerRadius::same(3),
            Stroke::new(if selected { 1.4 } else { 1.0 }, border_col),
            StrokeKind::Inside,
        );
    }
}

/// Renders the shared Page Size & Orientation picker (live preview, orientation switch, preset
/// card grid, and inline Custom dimensions bar). Returns `true` when any value changed.
pub fn render_page_format_picker(
    ui: &mut egui::Ui,
    t: &Tokens,
    draft: &mut PageFormatDraft,
    preview_ctx: &mut PageFormatPreviewContext<'_>,
) -> bool {
    let mut changed = false;
    let (eff_w, eff_h) = draft.effective_dimensions();
    let title_name = preset_display_title(draft.preset, draft.orientation);
    let badge_ratio = preset_ratio_badge(draft.preset, draft.orientation);

    // 1. Live Preview & Orientation Hero Card.
    card_frame(t).fill(t.toolbar_bg).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            let preview_box_size = vec2(84.0, 76.0);
            let (preview_bounds, _) = ui.allocate_exact_size(preview_box_size, Sense::hover());
            let sheet_rect =
                fit_proportional_rect(preview_bounds.shrink2(vec2(6.0, 4.0)), eff_w, eff_h);
            let shadow_rect = sheet_rect.translate(vec2(2.0, 2.5));
            ui.painter().rect_filled(
                shadow_rect,
                CornerRadius::same(4),
                egui::Color32::from_black_alpha(if t.is_dark { 85 } else { 26 }),
            );
            let turns = preview_ctx.preview_turns.rem_euclid(4);
            paint_paper_preview_colors_rotated(
                ui.painter(),
                sheet_rect,
                preview_ctx.paper_template,
                preview_ctx.paper_colors,
                t.accent,
                turns,
            );
            ui.painter().rect_stroke(
                sheet_rect,
                CornerRadius::same(4),
                Stroke::new(1.2, t.accent),
                StrokeKind::Outside,
            );

            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 5.0;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{title_name} · {}",
                            draft.orientation.label()
                        ))
                        .size(13.5)
                        .strong()
                        .color(t.text),
                    );
                    paint_badge_pill(
                        ui,
                        &format!("{eff_w} × {eff_h} pt"),
                        t.accent_soft,
                        t.accent,
                        t.accent,
                    );
                    paint_badge_pill(ui, badge_ratio, t.panel_bg, t.border, t.text_muted);
                    if turns != 0 {
                        paint_badge_pill(
                            ui,
                            &format!("{}°", turns * 90),
                            t.accent_soft,
                            t.accent,
                            t.accent,
                        );
                    }
                });

                ui.add(
                    egui::Label::new(
                        egui::RichText::new(draft.preset.description())
                            .size(11.0)
                            .color(t.text_muted),
                    )
                    .wrap(),
                );

                crate::theme::studio_wrapped_row(ui, vec2(8.0, 4.0), 28.0, |ui| {
                    crate::theme::studio_row_label(
                        ui,
                        egui::RichText::new("Orientation:")
                            .size(11.5)
                            .strong()
                            .color(t.text_muted),
                        28.0,
                    );
                    let options = [
                        SwitchOption::new(
                            None,
                            Some("Portrait"),
                            PageOrientation::Portrait.short_hint(),
                            true,
                        ),
                        SwitchOption::new(
                            None,
                            Some("Landscape"),
                            PageOrientation::Landscape.short_hint(),
                            true,
                        ),
                    ];
                    let active_idx = usize::from(draft.orientation == PageOrientation::Landscape);
                    let (picked, _) = segmented_switch(
                        ui,
                        (preview_ctx.id_prefix, "orientation_switch"),
                        &options,
                        Some(active_idx),
                        SwitchLayout::Compact,
                        t,
                    );
                    if let Some(picked) = picked {
                        let next_orient = if picked == 1 {
                            PageOrientation::Landscape
                        } else {
                            PageOrientation::Portrait
                        };
                        if next_orient != draft.orientation {
                            draft.set_orientation(next_orient);
                            changed = true;
                        }
                    }

                    if let Some(turns_ref) = preview_ctx.rotate_turns.as_deref_mut()
                        && compact_icon_button(
                            ui,
                            Icon::RotateCw,
                            Some("Rotate 90°"),
                            false,
                            true,
                            t,
                        )
                        .on_hover_text("Rotate existing page content and dimensions 90° clockwise")
                        .clicked()
                    {
                        *turns_ref = (*turns_ref + 1).rem_euclid(4);
                        draft.swap_dimensions();
                        changed = true;
                    }

                    if !draft.preset.supports_orientation() {
                        ui.label(
                            egui::RichText::new("(Square keeps equal sides)")
                                .size(10.5)
                                .color(t.text_muted),
                        );
                    } else if preview_ctx.custom_backdrop_active {
                        ui.label(
                            egui::RichText::new("(Template backdrop adapts cleanly)")
                                .size(10.5)
                                .color(t.text_muted),
                        );
                    }
                });
            });
        });
    });

    ui.add_space(10.0);

    // 2. Preset Visual Grid Card.
    section_card_header(
        ui,
        Icon::FitPage,
        "Sheet Size & Aspect Ratio",
        "Choose a paper standard, widescreen canvas, or custom resolution.",
        t,
    );
    ui.add_space(6.0);

    let cols = 4usize;
    let gap = 10.0_f32;
    let avail_w = ui.available_width();
    let card_w = ((avail_w - gap * ((cols - 1) as f32)) / (cols as f32)).floor();
    let card_h = 84.0_f32;

    for row in PageResolutionPreset::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &preset in row {
                let selected = draft.preset == preset;
                let tile_id = ui.make_persistent_id((preview_ctx.id_prefix, preset));
                let style = tile_motion_colors(ui, tile_id, selected, t);
                let (rect, resp) = ui.allocate_exact_size(vec2(card_w, card_h), Sense::click());
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                ui.ctx()
                    .data_mut(|d| d.insert_temp(tile_id.with("hov_mem"), resp.hovered()));
                let label = preset_display_title(preset, draft.orientation);
                let ratio = preset_ratio_badge(preset, draft.orientation);
                let (pw, ph) = if preset == PageResolutionPreset::Custom {
                    (
                        draft.custom_width.clamp(64, 8192),
                        draft.custom_height.clamp(64, 8192),
                    )
                } else {
                    preset.dimensions_with_orientation(draft.orientation)
                };
                ui.painter().rect(
                    rect,
                    CornerRadius::same(8),
                    style.fill,
                    style.stroke,
                    StrokeKind::Inside,
                );

                // Proportional miniature sheet inside the upper half of the card.
                let thumb_bounds = Rect::from_min_max(
                    pos2(rect.left() + 8.0, rect.top() + 7.0),
                    pos2(rect.right() - 8.0, rect.top() + 42.0),
                );
                paint_preset_mini_sheet(
                    ui.painter(),
                    thumb_bounds,
                    preset,
                    draft.orientation,
                    preview_ctx.paper_colors,
                    selected,
                    t,
                );

                // Preset ratio pill tucked cleanly in top-right corner.
                ui.painter().text(
                    pos2(rect.right() - 7.0, rect.top() + 7.0),
                    egui::Align2::RIGHT_TOP,
                    ratio,
                    egui::FontId::proportional(9.5),
                    if selected { t.accent } else { t.text_muted },
                );

                // Preset title + dimensions cleanly stacked at the bottom of the card.
                ui.painter().text(
                    pos2(rect.center().x, rect.bottom() - 25.0),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(11.5),
                    if selected { t.text } else { t.text_muted },
                );
                let dim_text = if preset == PageResolutionPreset::Custom {
                    format!("{pw} × {ph}")
                } else {
                    format!("{pw} × {ph} pt")
                };
                ui.painter().text(
                    pos2(rect.center().x, rect.bottom() - 11.0),
                    egui::Align2::CENTER_CENTER,
                    dim_text,
                    egui::FontId::proportional(10.0),
                    if selected { t.accent } else { t.text_muted },
                );

                let hover_text = if preset == PageResolutionPreset::Custom {
                    format!("{label}: {}", preset.description())
                } else {
                    format!("{label} ({pw} × {ph} pt): {}", preset.description())
                };
                if resp.on_hover_text(hover_text).clicked() {
                    draft.select_preset(preset);
                    changed = true;
                }
            }
        });
        ui.add_space(6.0);
    }

    // 3. Inline Custom Width / Height Controls (animated open/close when switching Custom Size).
    let custom_open = draft.preset == PageResolutionPreset::Custom;
    let custom_id = ui.make_persistent_id((preview_ctx.id_prefix, "custom_dims_collapse"));
    crate::motion::animated_collapse(ui, custom_id, custom_open, 42.0, |ui| {
        egui::Frame::NONE
            .fill(t.toolbar_bg)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(
                        egui::RichText::new("Custom Dimensions")
                            .size(11.5)
                            .strong()
                            .color(t.text),
                    );
                    let mut w_val = draft.custom_width.clamp(64, 8192);
                    let mut h_val = draft.custom_height.clamp(64, 8192);
                    ui.label(egui::RichText::new("Width").size(11.0).color(t.text_muted));
                    let w_resp = ui.add(
                        egui::DragValue::new(&mut w_val)
                            .range(64..=8192)
                            .speed(8.0)
                            .suffix(" pt"),
                    );
                    ui.label(egui::RichText::new("×").size(11.5).color(t.text_muted));
                    ui.label(egui::RichText::new("Height").size(11.0).color(t.text_muted));
                    let h_resp = ui.add(
                        egui::DragValue::new(&mut h_val)
                            .range(64..=8192)
                            .speed(8.0)
                            .suffix(" pt"),
                    );
                    if w_resp.changed() || h_resp.changed() {
                        draft.set_custom_dimensions(w_val, h_val);
                        changed = true;
                    }
                    if compact_icon_button(ui, Icon::RotateCw, None, false, true, t)
                        .on_hover_text("Swap custom width and height")
                        .clicked()
                    {
                        draft.swap_dimensions();
                        changed = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let _ = ui.label(
                            egui::RichText::new("Range: 64–8192 pt")
                                .size(10.5)
                                .color(t.text_muted),
                        );
                    });
                });
            });
    });
    if !custom_open {
        let _ = ui.label(
            egui::RichText::new(
                "Tip: Portrait or Landscape adjusts the sheet shape while keeping strokes upright.",
            )
            .size(10.5)
            .color(t.text_muted),
        );
    }

    changed
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn page_format_draft_detects_presets_orientations_and_swaps_cleanly() {
        let a4_landscape = PageFormatDraft::from_dimensions(1684, 1190);
        assert_eq!(a4_landscape.preset, PageResolutionPreset::A4);
        assert_eq!(a4_landscape.orientation, PageOrientation::Landscape);
        assert_eq!(a4_landscape.effective_dimensions(), (1684, 1190));

        let mut draft = PageFormatDraft::default_standard();
        assert_eq!(draft.effective_dimensions(), (1200, 1600));

        draft.set_orientation(PageOrientation::Landscape);
        assert_eq!(draft.effective_dimensions(), (1600, 1200));

        draft.select_preset(PageResolutionPreset::A5Compact);
        assert_eq!(draft.effective_dimensions(), (1190, 840));

        draft.set_orientation(PageOrientation::Portrait);
        assert_eq!(draft.effective_dimensions(), (840, 1190));

        draft.set_custom_dimensions(2000, 1000);
        assert_eq!(draft.preset, PageResolutionPreset::Custom);
        assert_eq!(draft.orientation, PageOrientation::Landscape);
        assert_eq!(draft.effective_dimensions(), (2000, 1000));

        draft.swap_dimensions();
        assert_eq!(draft.orientation, PageOrientation::Portrait);
        assert_eq!(draft.effective_dimensions(), (1000, 2000));
    }

    #[test]
    fn page_format_picker_renders_cleanly_in_light_and_dark_and_handles_clicks() {
        let ctx = egui::Context::default();
        let mut draft = PageFormatDraft::default_standard();
        let colors = drawcraft_doc::PaperColor::WarmCream
            .resolved_colors(drawcraft_doc::UiTheme::Light, false);

        let run_picker = |theme: drawcraft_doc::UiTheme,
                          draft: &mut PageFormatDraft,
                          events: Vec<egui::Event>,
                          time: f64| {
            let t = if matches!(theme, drawcraft_doc::UiTheme::Dark) {
                Tokens::dark()
            } else {
                Tokens::light()
            };
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(720.0, 520.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                if matches!(theme, drawcraft_doc::UiTheme::Dark) {
                    crate::theme::apply_visuals(ui.ctx(), drawcraft_doc::UiTheme::Dark);
                } else {
                    crate::theme::apply_light_visuals(ui.ctx());
                }
                ui.set_width(642.0);
                let mut preview_ctx = PageFormatPreviewContext {
                    id_prefix: "picker_test",
                    paper_colors: colors,
                    paper_template: PaperTemplate::Grid,
                    preview_turns: 0,
                    custom_backdrop_active: false,
                    rotate_turns: None,
                };
                render_page_format_picker(ui, &t, draft, &mut preview_ctx);
            });
            let shapes = out.shapes.clone();
            out.drop_without_applying_deltas();
            shapes
        };

        let find_text = |shapes: &[egui::epaint::ClippedShape],
                         needle: &str|
         -> Option<egui::Pos2> {
            fn visit(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
                match shape {
                    egui::Shape::Text(text) if text.galley.text() == needle => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    egui::Shape::Vec(children) => children.iter().find_map(|c| visit(c, needle)),
                    _ => None,
                }
            }
            shapes.iter().find_map(|c| visit(&c.shape, needle))
        };

        let mut shapes = run_picker(drawcraft_doc::UiTheme::Light, &mut draft, Vec::new(), 0.05);
        for expected in [
            "Standard",
            "A4",
            "US Letter",
            "A5 Compact",
            "Square",
            "Custom Size",
        ] {
            assert!(
                find_text(&shapes, expected).is_some(),
                "missing preset card {expected:?}"
            );
        }

        // Click A4 card -> switch to Landscape -> confirm 1684 × 1190 pt.
        let a4_pos = find_text(&shapes, "A4").expect("A4 card");
        for (pressed, t) in [(true, 0.10), (false, 0.12)] {
            let _ = run_picker(
                drawcraft_doc::UiTheme::Light,
                &mut draft,
                vec![
                    egui::Event::PointerMoved(a4_pos),
                    egui::Event::PointerButton {
                        pos: a4_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                t,
            );
        }
        shapes = run_picker(drawcraft_doc::UiTheme::Light, &mut draft, Vec::new(), 0.15);
        assert_eq!(draft.preset, PageResolutionPreset::A4);

        let land_pos = find_text(&shapes, "Landscape").expect("Landscape toggle");
        for (pressed, t) in [(true, 0.20), (false, 0.22)] {
            let _ = run_picker(
                drawcraft_doc::UiTheme::Dark,
                &mut draft,
                vec![
                    egui::Event::PointerMoved(land_pos),
                    egui::Event::PointerButton {
                        pos: land_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                t,
            );
        }
        assert_eq!(draft.orientation, PageOrientation::Landscape);
        assert_eq!(draft.effective_dimensions(), (1684, 1190));

        shapes = run_picker(drawcraft_doc::UiTheme::Dark, &mut draft, Vec::new(), 0.25);
        assert!(find_text(&shapes, "1684 × 1190 pt").is_some());
        assert!(find_text(&shapes, "16:9 Wide").is_some());
    }
}
