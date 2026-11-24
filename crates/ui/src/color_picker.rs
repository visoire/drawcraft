//! Floating color picker, swatch editor, and palette manager popover for Ink and Highlighter colors.

use drawcraft_color::Color;
use drawcraft_doc::{
    ColorPalettePreset, MAX_PALETTE_COLORS, MAX_PALETTE_NAME_LEN, MAX_PALETTE_PRESETS,
    PaletteTarget, PenKind,
};
use drawcraft_engine::ToolKind;
use egui::epaint::{Vertex, WHITE_UV};
use egui::{
    Color32, CornerRadius, Frame, Margin, Mesh, Order, Rect, RichText, Sense, Stroke, StrokeKind,
    Vec2, pos2,
};
use serde_json::json;

use crate::DrawCraftApp;
use crate::icons::{Icon, compact_icon_button, icon_button, paint_icon};
use crate::theme::{Tokens, to_color32};

const POPOVER_WIDTH: f32 = 288.0;
const SV_BOX_HEIGHT_WILDCARD: f32 = 128.0;
const SV_BOX_HEIGHT_SWATCH: f32 = 124.0;
const SV_BOX_HEIGHT_EDITOR: f32 = 114.0;
const HUE_BAR_HEIGHT: f32 = 14.0;

pub(crate) fn same_rgb(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 0.02 && (a.g - b.g).abs() < 0.02 && (a.b - b.b).abs() < 0.02
}

fn palette_target_str(target: PaletteTarget) -> &'static str {
    match target {
        PaletteTarget::Ink => "ink",
        PaletteTarget::Highlighter => "highlighter",
    }
}

/// Active screen displayed inside the floating color popover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorPopoverMode {
    /// Pure color picker for the standalone wildcard color (never touches palettes).
    Wildcard,
    /// Quick swatch editor opened by right-clicking a custom palette dot in the dock.
    EditSwatch {
        preset_id: String,
        slot_index: usize,
    },
    /// Palette switcher and manager opened from the dock's Palettes button.
    Palettes,
    /// Full custom palette editor (rename, add/remove/edit swatches, delete palette).
    PaletteEditor {
        preset_id: String,
        selected_slot: usize,
        confirm_delete: bool,
    },
}

/// Interactive state for the floating color picker & palette popover.
#[derive(Clone, Debug)]
pub struct ColorPickerState {
    pub open: bool,
    pub mode: ColorPopoverMode,
    pub target: PaletteTarget,
    /// `[hue (0..=1), saturation (0..=1), value (0..=1)]`.
    pub hsv: [f32; 3],
    pub hex_draft: String,
    pub anchor_rect: Rect,
    opened_this_frame: bool,
    pub preset_name_draft: String,
    request_focus_name_edit: bool,
}

impl Default for ColorPickerState {
    fn default() -> Self {
        let initial = Color::from_hex("#141923").unwrap_or(Color::BLACK);
        Self {
            open: false,
            mode: ColorPopoverMode::Wildcard,
            target: PaletteTarget::Ink,
            hsv: initial.to_hsv(),
            hex_draft: initial.to_hex(),
            anchor_rect: Rect::NOTHING,
            opened_this_frame: false,
            preset_name_draft: String::new(),
            request_focus_name_edit: false,
        }
    }
}

impl ColorPickerState {
    /// Opens the pure standalone wildcard color picker anchored under `anchor_rect`.
    pub fn open_wildcard(&mut self, kind: PenKind, color: Color, anchor_rect: Rect) {
        self.target = PaletteTarget::for_pen(kind);
        self.mode = ColorPopoverMode::Wildcard;
        self.clear_name_draft();
        self.sync_from_color(color);
        self.anchor_rect = anchor_rect;
        self.open = true;
        self.opened_this_frame = true;
    }

    /// Opens the quick swatch editor for `slot_index` in custom palette `preset_id`.
    pub fn open_edit_swatch(
        &mut self,
        target: PaletteTarget,
        preset_id: String,
        slot_index: usize,
        color: Color,
        anchor_rect: Rect,
    ) {
        self.target = target;
        self.mode = ColorPopoverMode::EditSwatch {
            preset_id,
            slot_index,
        };
        self.clear_name_draft();
        self.sync_from_color(color);
        self.anchor_rect = anchor_rect;
        self.open = true;
        self.opened_this_frame = true;
    }

    /// Opens the palette switcher/manager popover anchored under `anchor_rect`.
    pub fn open_palettes(&mut self, kind: PenKind, color: Color, anchor_rect: Rect) {
        self.target = PaletteTarget::for_pen(kind);
        self.mode = ColorPopoverMode::Palettes;
        self.clear_name_draft();
        self.sync_from_color(color);
        self.anchor_rect = anchor_rect;
        self.open = true;
        self.opened_this_frame = true;
    }

    #[must_use]
    pub fn is_wildcard_mode(&self) -> bool {
        self.open && matches!(self.mode, ColorPopoverMode::Wildcard)
    }

    #[must_use]
    pub fn is_palette_mode(&self) -> bool {
        self.open
            && matches!(
                self.mode,
                ColorPopoverMode::Palettes | ColorPopoverMode::PaletteEditor { .. }
            )
    }

    pub fn close(&mut self) {
        self.open = false;
        self.opened_this_frame = false;
        self.clear_name_draft();
    }

    fn clear_name_draft(&mut self) {
        self.preset_name_draft.clear();
        self.request_focus_name_edit = false;
    }

    pub fn sync_from_color(&mut self, color: Color) {
        let opaque = color.with_alpha(1.0);
        self.hsv = opaque.to_hsv();
        self.hex_draft = opaque.to_hex();
    }

    pub fn current_color(&self) -> Color {
        Color::from_hsv(self.hsv[0], self.hsv[1], self.hsv[2], 1.0)
    }
}

/// Renders the custom color picker popover when open.
pub fn show_popover(app: &mut DrawCraftApp, ui: &mut egui::Ui) {
    if !app.view.color_picker.open {
        return;
    }

    let ctx = ui.ctx().clone();
    let t = Tokens::for_theme(app.session.prefs.theme);
    let viewport = ctx.content_rect();
    let just_opened = app.view.color_picker.opened_this_frame;
    app.view.color_picker.opened_this_frame = false;

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if let ColorPopoverMode::PaletteEditor {
            confirm_delete: ref mut confirm,
            ..
        } = app.view.color_picker.mode
            && *confirm
        {
            *confirm = false;
            return;
        }
        commit_palette_name_draft(app);
        app.view.color_picker.close();
        return;
    }

    let est_height = match &app.view.color_picker.mode {
        ColorPopoverMode::Wildcard => 270.0,
        ColorPopoverMode::EditSwatch { .. } => 315.0,
        ColorPopoverMode::Palettes => 380.0,
        ColorPopoverMode::PaletteEditor { .. } => 450.0,
    };
    let anchor = app.view.color_picker.anchor_rect;
    let margin = 12_i8;
    let total_w = POPOVER_WIDTH + f32::from(margin) * 2.0;
    let dock_rect = app.view.color_dock.last_rect;
    let from_color_dock = dock_rect.is_positive()
        && anchor != Rect::NOTHING
        && dock_rect.intersects(anchor.expand(4.0));
    let (desired_x, desired_y) = if anchor == Rect::NOTHING {
        (viewport.center().x - total_w * 0.5, viewport.top() + 52.0)
    } else if from_color_dock {
        let canvas_bounds = app
            .view
            .last_canvas_rect
            .filter(|r| r.is_positive())
            .unwrap_or(viewport);
        let left_candidate = dock_rect.left() - total_w - 10.0;
        let right_candidate = dock_rect.right() + 10.0;
        let prefer_right = dock_rect.center().x <= canvas_bounds.center().x;
        let x = if prefer_right {
            if right_candidate + total_w <= viewport.right() - 8.0 {
                right_candidate
            } else if left_candidate >= viewport.left() + 8.0 {
                left_candidate
            } else {
                viewport.center().x - total_w * 0.5
            }
        } else if left_candidate >= viewport.left() + 8.0 {
            left_candidate
        } else if right_candidate + total_w <= viewport.right() - 8.0 {
            right_candidate
        } else {
            viewport.center().x - total_w * 0.5
        };
        (x, anchor.center().y - est_height * 0.5)
    } else {
        (anchor.left(), anchor.bottom() + 6.0)
    };
    let clamped_x = desired_x.clamp(
        viewport.left() + 8.0,
        (viewport.right() - total_w - 8.0).max(viewport.left() + 8.0),
    );
    let clamped_y = desired_y.clamp(
        viewport.top() + 8.0,
        (viewport.bottom() - est_height - 16.0).max(viewport.top() + 8.0),
    );

    let area_id = ui.make_persistent_id("drawcraft_color_picker_popover");
    let shadow = ctx.style_of(ctx.theme()).visuals.window_shadow;
    let frame = Frame::NONE
        .fill(t.field_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(margin))
        .shadow(shadow);

    let area_resp = egui::Area::new(area_id)
        .order(Order::Foreground)
        .fixed_pos(pos2(clamped_x, clamped_y))
        .constrain_to(viewport.shrink(6.0))
        .show(&ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(POPOVER_WIDTH);
                render_popover_contents(app, ui, &t);
            });
        });

    let clicked_outside = !just_opened
        && ctx.input(|i| {
            i.pointer.primary_pressed()
                && i.pointer.interact_pos().is_some_and(|p| {
                    !area_resp.response.rect.contains(p)
                        && !(dock_rect.is_positive() && dock_rect.contains(p))
                })
        });
    if clicked_outside {
        commit_palette_name_draft(app);
        app.view.color_picker.close();
    }
}

fn render_popover_contents(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    match app.view.color_picker.mode.clone() {
        ColorPopoverMode::Wildcard => render_wildcard_mode(app, ui, t),
        ColorPopoverMode::EditSwatch {
            preset_id,
            slot_index,
        } => render_edit_swatch_mode(app, ui, preset_id, slot_index, t),
        ColorPopoverMode::Palettes => render_palettes_mode(app, ui, t),
        ColorPopoverMode::PaletteEditor {
            preset_id,
            selected_slot,
            confirm_delete,
        } => render_palette_editor_mode(app, ui, preset_id, selected_slot, confirm_delete, t),
    }
}

fn draw_popover_header(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    back_tooltip: Option<&str>,
    t: &Tokens,
) -> (bool, bool) {
    let mut back_clicked = false;
    let mut close_clicked = false;
    ui.horizontal(|ui| {
        if let Some(tip) = back_tooltip
            && compact_icon_button(ui, Icon::PrevPage, None, false, true, t)
                .on_hover_text(tip)
                .clicked()
        {
            back_clicked = true;
        }
        ui.label(RichText::new(title).size(13.0).strong().color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, Icon::Close, None, false, true, t)
                .on_hover_text("Close color picker (Esc)")
                .clicked()
            {
                close_clicked = true;
            }
        });
    });
    ui.add(egui::Label::new(RichText::new(subtitle).size(10.5).color(t.text_muted)).wrap());
    (back_clicked, close_clicked)
}

fn render_wildcard_mode(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let target = app.view.color_picker.target;
    let (title, subtitle) = match target {
        PaletteTarget::Ink => (
            "Custom Color",
            "Standalone color for all pens — independent of palettes.",
        ),
        PaletteTarget::Highlighter => (
            "Highlighter Custom Color",
            "Standalone highlighter color — independent of palettes.",
        ),
    };
    let (_, close_clicked) = draw_popover_header(ui, title, subtitle, None, t);
    if close_clicked {
        app.view.color_picker.close();
        return;
    }
    ui.add_space(8.0);

    let mut color_changed =
        draw_sv_square(ui, &mut app.view.color_picker, SV_BOX_HEIGHT_WILDCARD, t);
    ui.add_space(6.0);
    color_changed |= draw_hue_bar(ui, &mut app.view.color_picker, t);
    ui.add_space(8.0);
    color_changed |= draw_preview_and_hex_row(ui, &mut app.view.color_picker, t);

    if color_changed {
        let picked = app.view.color_picker.current_color();
        let _ = app.session.execute(
            "palette.setWildcard",
            &json!({
                "target": palette_target_str(target),
                "color": picked,
                "apply_to_pen": false,
            }),
        );
        app.view.color_dock.selected_wildcard = Some(target);
        apply_color_to_target(app, picked);
    }
}

fn render_edit_swatch_mode(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    preset_id: String,
    slot_index: usize,
    t: &Tokens,
) {
    let target = app.view.color_picker.target;
    let Some(preset) = app.session.prefs.palette_by_id(&preset_id).cloned() else {
        app.view.color_picker.close();
        return;
    };
    let swatches = preset.colors(target);
    if preset.built_in || slot_index >= swatches.len() {
        app.view.color_picker.close();
        return;
    }
    let palette_len = swatches.len();

    let title = format!("{} · Swatch #{}", preset.name, slot_index + 1);
    let (_, close_clicked) = draw_popover_header(
        ui,
        &title,
        "Adjust or remove this swatch in your custom palette.",
        None,
        t,
    );
    if close_clicked {
        app.view.color_picker.close();
        return;
    }
    ui.add_space(8.0);

    let mut color_changed = draw_sv_square(ui, &mut app.view.color_picker, SV_BOX_HEIGHT_SWATCH, t);
    ui.add_space(6.0);
    color_changed |= draw_hue_bar(ui, &mut app.view.color_picker, t);
    ui.add_space(8.0);
    color_changed |= draw_preview_and_hex_row(ui, &mut app.view.color_picker, t);

    if color_changed {
        let picked = app.view.color_picker.current_color();
        let _ = app.session.execute(
            "palette.setColor",
            &json!({
                "preset_id": preset_id,
                "target": palette_target_str(target),
                "index": slot_index,
                "color": picked,
                "apply_to_pen": false,
            }),
        );
        apply_color_to_target(app, picked);
    }

    ui.add_space(8.0);
    let can_remove = palette_len > 1;
    let mut remove_clicked = false;
    let mut edit_palette_clicked = false;
    ui.horizontal(|ui| {
        let remove_tip = if can_remove {
            "Remove this swatch from the palette"
        } else {
            "Keep at least one swatch in the palette"
        };
        if draw_danger_button(ui, Some(Icon::Trash), "Remove", false, can_remove, t)
            .on_hover_text(remove_tip)
            .clicked()
        {
            remove_clicked = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, Icon::Edit, Some("Edit Palette"), false, true, t)
                .on_hover_text("Open the full palette editor")
                .clicked()
            {
                edit_palette_clicked = true;
            }
        });
    });

    if remove_clicked
        && app
            .session
            .execute(
                "palette.removeColor",
                &json!({
                    "preset_id": preset_id,
                    "target": palette_target_str(target),
                    "index": slot_index,
                }),
            )
            .is_ok()
    {
        let remaining = app
            .session
            .prefs
            .palette_by_id(&preset_id)
            .map(|p| p.colors(target).to_vec())
            .unwrap_or_default();
        if remaining.is_empty() {
            app.view.color_picker.close();
        } else {
            let clamped = slot_index.min(remaining.len() - 1);
            let next_color = remaining[clamped];
            app.view.color_picker.mode = ColorPopoverMode::EditSwatch {
                preset_id,
                slot_index: clamped,
            };
            app.view.color_picker.sync_from_color(next_color);
            apply_color_to_target(app, next_color);
        }
    } else if edit_palette_clicked {
        app.view.color_picker.preset_name_draft = preset.name;
        app.view.color_picker.request_focus_name_edit = false;
        app.view.color_picker.mode = ColorPopoverMode::PaletteEditor {
            preset_id,
            selected_slot: slot_index,
            confirm_delete: false,
        };
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaletteRowAction {
    None,
    Select,
    Duplicate,
    Edit,
}

fn render_palettes_mode(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let target = app.view.color_picker.target;
    let (title, subtitle) = match target {
        PaletteTarget::Ink => (
            "Ink Palettes",
            "Switch active swatches or create your own palettes.",
        ),
        PaletteTarget::Highlighter => (
            "Highlighter Palettes",
            "Switch active swatches or create your own palettes.",
        ),
    };
    let (_, close_clicked) = draw_popover_header(ui, title, subtitle, None, t);
    if close_clicked {
        app.view.color_picker.close();
        return;
    }
    ui.add_space(8.0);

    let presets = app.session.prefs.palette_presets.clone();
    let active_id = app.session.prefs.active_palette_id.clone();
    let can_add_preset = presets.len() < MAX_PALETTE_PRESETS;
    let builtins: Vec<&ColorPalettePreset> = presets.iter().filter(|p| p.built_in).collect();
    let customs: Vec<&ColorPalettePreset> = presets.iter().filter(|p| !p.built_in).collect();

    let mut chosen_action: Option<(String, String, PaletteRowAction)> = None;

    ui.label(
        RichText::new("Built-in")
            .size(10.5)
            .strong()
            .color(t.text_muted),
    );
    ui.add_space(4.0);
    for preset in builtins {
        let act = draw_palette_row(
            ui,
            preset,
            target,
            preset.id == active_id,
            can_add_preset,
            t,
        );
        if act != PaletteRowAction::None {
            chosen_action = Some((preset.id.clone(), preset.name.clone(), act));
        }
        ui.add_space(4.0);
    }

    ui.add_space(4.0);
    let custom_heading = if customs.is_empty() {
        "My Palettes".to_string()
    } else {
        format!("My Palettes ({})", customs.len())
    };
    ui.label(
        RichText::new(custom_heading)
            .size(10.5)
            .strong()
            .color(t.text_muted),
    );
    ui.add_space(4.0);
    if customs.is_empty() {
        ui.label(
            RichText::new("No custom palettes yet.")
                .size(11.0)
                .color(t.text_muted),
        );
    } else {
        egui::ScrollArea::vertical()
            .id_salt("color_picker_custom_palettes")
            .max_height(150.0)
            .show(ui, |ui| {
                ui.set_width(POPOVER_WIDTH);
                for preset in customs {
                    let act = draw_palette_row(
                        ui,
                        preset,
                        target,
                        preset.id == active_id,
                        can_add_preset,
                        t,
                    );
                    if act != PaletteRowAction::None {
                        chosen_action = Some((preset.id.clone(), preset.name.clone(), act));
                    }
                    ui.add_space(4.0);
                }
            });
    }

    ui.add_space(8.0);
    let new_tip = if can_add_preset {
        "Create a new custom color palette"
    } else {
        "Maximum number of palettes reached"
    };
    let create_clicked = icon_button(
        ui,
        Icon::Plus,
        Some("New Palette"),
        false,
        can_add_preset,
        t,
    )
    .on_hover_text(new_tip)
    .clicked();

    if let Some((preset_id, preset_name, act)) = chosen_action {
        match act {
            PaletteRowAction::Select => {
                if app
                    .session
                    .execute("palette.selectPreset", &json!({ "id": preset_id }))
                    .is_ok()
                {
                    app.view.color_dock.selected_wildcard = None;
                    if let Some(&first) = app.session.prefs.palette(target).first() {
                        app.view.color_picker.sync_from_color(first);
                        apply_color_to_target(app, first);
                    }
                }
            }
            PaletteRowAction::Duplicate => {
                let dup_name = next_unique_palette_name(&presets, &format!("{preset_name} Copy"));
                if let Ok(res) = app.session.execute(
                    "palette.savePreset",
                    &json!({
                        "name": dup_name,
                        "copy_from_id": preset_id,
                        "select": true,
                    }),
                ) && let Some(new_id) = res["id"].as_str().map(str::to_string)
                {
                    let saved_name = app
                        .session
                        .prefs
                        .palette_by_id(&new_id)
                        .map_or(dup_name, |p| p.name.clone());
                    app.view.color_dock.selected_wildcard = None;
                    app.view.color_picker.preset_name_draft = saved_name;
                    app.view.color_picker.request_focus_name_edit = true;
                    app.view.color_picker.mode = ColorPopoverMode::PaletteEditor {
                        preset_id: new_id,
                        selected_slot: 0,
                        confirm_delete: false,
                    };
                    if let Some(&first) = app.session.prefs.palette(target).first() {
                        app.view.color_picker.sync_from_color(first);
                        apply_color_to_target(app, first);
                    }
                }
            }
            PaletteRowAction::Edit => {
                if app
                    .session
                    .execute("palette.selectPreset", &json!({ "id": preset_id }))
                    .is_ok()
                {
                    app.view.color_dock.selected_wildcard = None;
                    app.view.color_picker.preset_name_draft = preset_name;
                    app.view.color_picker.request_focus_name_edit = false;
                    app.view.color_picker.mode = ColorPopoverMode::PaletteEditor {
                        preset_id,
                        selected_slot: 0,
                        confirm_delete: false,
                    };
                    if let Some(&first) = app.session.prefs.palette(target).first() {
                        app.view.color_picker.sync_from_color(first);
                        apply_color_to_target(app, first);
                    }
                }
            }
            PaletteRowAction::None => {}
        }
    } else if create_clicked {
        let new_name = next_unique_palette_name(&presets, "My Palette");
        let seed_hex = app.view.color_picker.current_color().to_hex();
        if let Ok(res) = app.session.execute(
            "palette.savePreset",
            &json!({
                "name": new_name,
                "copy_from_active": false,
                "seed_color_hex": seed_hex,
                "select": true,
            }),
        ) && let Some(new_id) = res["id"].as_str().map(str::to_string)
        {
            let saved_name = app
                .session
                .prefs
                .palette_by_id(&new_id)
                .map_or(new_name, |p| p.name.clone());
            app.view.color_dock.selected_wildcard = None;
            app.view.color_picker.preset_name_draft = saved_name;
            app.view.color_picker.request_focus_name_edit = true;
            app.view.color_picker.mode = ColorPopoverMode::PaletteEditor {
                preset_id: new_id,
                selected_slot: 0,
                confirm_delete: false,
            };
            if let Some(&first) = app.session.prefs.palette(target).first() {
                app.view.color_picker.sync_from_color(first);
                apply_color_to_target(app, first);
            }
        }
    }
}

fn draw_palette_row(
    ui: &mut egui::Ui,
    preset: &ColorPalettePreset,
    target: PaletteTarget,
    is_active: bool,
    can_duplicate: bool,
    t: &Tokens,
) -> PaletteRowAction {
    let (row_rect, _) = ui.allocate_exact_size(Vec2::new(POPOVER_WIDTH, 34.0), Sense::hover());
    let action_rect = Rect::from_center_size(
        pos2(row_rect.right() - 17.0, row_rect.center().y),
        Vec2::splat(26.0),
    );
    let select_rect = Rect::from_min_max(
        row_rect.min,
        pos2(action_rect.left() - 4.0, row_rect.bottom()),
    );

    let sel_id = ui.make_persistent_id(("pal_row_sel", &preset.id));
    let act_id = ui.make_persistent_id(("pal_row_act", &preset.id));
    let sel_resp = ui.interact(select_rect, sel_id, Sense::click());
    let act_enabled = !preset.built_in || can_duplicate;
    let act_sense = if act_enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let act_resp = ui.interact(action_rect, act_id, act_sense);

    let colors = preset.colors(target);
    if ui.is_rect_visible(row_rect) {
        let hov_t = crate::motion::hover_factor(
            ui.ctx(),
            sel_id.with("hov"),
            sel_resp.hovered() && !is_active,
        );
        let fill = if is_active {
            t.accent_soft
        } else {
            crate::motion::lerp_color(t.panel_bg, t.border.gamma_multiply(0.45), hov_t)
        };
        let border = if is_active {
            t.accent
        } else {
            crate::motion::lerp_color(t.border, t.text_muted, hov_t * 0.5)
        };
        let p = ui.painter();
        p.rect_filled(row_rect, CornerRadius::same(7), fill);
        p.rect_stroke(
            row_rect,
            CornerRadius::same(7),
            Stroke::new(if is_active { 1.3 } else { 1.0 }, border),
            StrokeKind::Inside,
        );

        let text_left = if is_active {
            let check_rect = Rect::from_center_size(
                pos2(select_rect.left() + 14.0, select_rect.center().y),
                Vec2::splat(13.0),
            );
            paint_icon(p, check_rect, Icon::Check, t.accent);
            select_rect.left() + 25.0
        } else {
            select_rect.left() + 10.0
        };

        let preview_count = colors.len().min(6);
        let dots_w = if preview_count > 0 {
            preview_count as f32 * 8.0 + 4.0
        } else {
            0.0
        };
        let max_text_w = (select_rect.right() - text_left - dots_w - 6.0).max(24.0);
        let text_color = if is_active { t.accent } else { t.text };
        let mut job = egui::text::LayoutJob::single_section(
            preset.name.clone(),
            egui::TextFormat {
                font_id: egui::FontId::proportional(11.8),
                color: text_color,
                ..Default::default()
            },
        );
        job.wrap.max_width = max_text_w;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = p.layout_job(job);
        p.galley(
            pos2(text_left, select_rect.center().y - galley.size().y * 0.5),
            galley,
            text_color,
        );

        let rim = if t.is_dark {
            Color32::from_white_alpha(44)
        } else {
            Color32::from_black_alpha(36)
        };
        let mut dot_x = select_rect.right() - 7.0;
        for &c in colors.iter().take(preview_count).rev() {
            let c_pos = pos2(dot_x, select_rect.center().y);
            p.circle_filled(c_pos, 3.2, to_color32(c.with_alpha(1.0)));
            p.circle_stroke(c_pos, 3.2, Stroke::new(0.6, rim));
            dot_x -= 8.0;
        }

        let act_hov = if act_enabled {
            crate::motion::hover_factor(ui.ctx(), act_id.with("hov"), act_resp.hovered())
        } else {
            0.0
        };
        if act_hov > 0.01 {
            p.rect_filled(
                action_rect,
                CornerRadius::same(5),
                t.border.gamma_multiply(0.45 * act_hov),
            );
        }
        let icon = if preset.built_in {
            Icon::Duplicate
        } else {
            Icon::Edit
        };
        let icon_color = if !act_enabled {
            t.text_muted.gamma_multiply(0.45)
        } else {
            crate::motion::lerp_color(t.text_muted, t.text, act_hov)
        };
        paint_icon(
            p,
            Rect::from_center_size(action_rect.center(), Vec2::splat(14.0)),
            icon,
            icon_color,
        );
    }

    let sel_tip = format!(
        "{} ({} swatches) — Click to apply",
        preset.name,
        colors.len()
    );
    let act_tip = if preset.built_in {
        if can_duplicate {
            "Duplicate into a custom palette"
        } else {
            "Maximum number of palettes reached"
        }
    } else {
        "Edit swatches or rename palette"
    };
    let sel_clicked = sel_resp.on_hover_text(sel_tip).clicked();
    let act_clicked = act_resp.on_hover_text(act_tip).clicked();

    if act_clicked && act_enabled {
        if preset.built_in {
            PaletteRowAction::Duplicate
        } else {
            PaletteRowAction::Edit
        }
    } else if sel_clicked {
        PaletteRowAction::Select
    } else {
        PaletteRowAction::None
    }
}

fn render_palette_editor_mode(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    preset_id: String,
    selected_slot: usize,
    confirm_delete: bool,
    t: &Tokens,
) {
    let target = app.view.color_picker.target;
    let Some(preset) = app.session.prefs.palette_by_id(&preset_id).cloned() else {
        app.view.color_picker.mode = ColorPopoverMode::Palettes;
        return;
    };
    let swatches = preset.colors(target).to_vec();
    if preset.built_in || swatches.is_empty() {
        app.view.color_picker.mode = ColorPopoverMode::Palettes;
        return;
    }

    let mut active_slot = selected_slot.min(swatches.len() - 1);
    let mut next_confirm = confirm_delete;

    let subtitle = match target {
        PaletteTarget::Ink => "Editing ink swatches — changes save automatically.",
        PaletteTarget::Highlighter => "Editing highlighter swatches — changes save automatically.",
    };
    let (back_clicked, close_clicked) =
        draw_popover_header(ui, "Edit Palette", subtitle, Some("Back to palettes"), t);
    if back_clicked {
        commit_palette_name_draft(app);
        app.view.color_picker.clear_name_draft();
        app.view.color_picker.mode = ColorPopoverMode::Palettes;
        return;
    }
    if close_clicked {
        commit_palette_name_draft(app);
        app.view.color_picker.close();
        return;
    }
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        crate::theme::studio_row_label(
            ui,
            RichText::new("Name").size(11.5).color(t.text_muted),
            28.0,
        );
        let edit_w = (POPOVER_WIDTH - 46.0).max(120.0);
        let edit_resp = ui.add_sized(
            [edit_w, 28.0],
            egui::TextEdit::singleline(&mut app.view.color_picker.preset_name_draft)
                .desired_width(edit_w)
                .char_limit(MAX_PALETTE_NAME_LEN)
                .margin(egui::Margin::symmetric(8, 2))
                .align(egui::Align2::LEFT_CENTER)
                .hint_text("Palette name..."),
        );
        if app.view.color_picker.request_focus_name_edit {
            edit_resp.request_focus();
            app.view.color_picker.request_focus_name_edit = false;
        }
        if edit_resp.lost_focus() {
            commit_palette_name_draft(app);
        }
    });

    ui.add_space(6.0);
    let swatches_label = match target {
        PaletteTarget::Ink => format!("Ink Swatches ({}/{MAX_PALETTE_COLORS})", swatches.len()),
        PaletteTarget::Highlighter => {
            format!(
                "Highlighter Swatches ({}/{MAX_PALETTE_COLORS})",
                swatches.len()
            )
        }
    };
    let can_remove = swatches.len() > 1;
    let mut remove_slot_clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(swatches_label).size(11.0).color(t.text_muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tip = if can_remove {
                "Remove selected swatch"
            } else {
                "Keep at least one swatch"
            };
            if compact_icon_button(ui, Icon::Trash, None, false, can_remove, t)
                .on_hover_text(tip)
                .clicked()
            {
                remove_slot_clicked = true;
            }
        });
    });

    ui.add_space(4.0);
    let mut clicked_slot: Option<usize> = None;
    let mut add_slot_clicked = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
        for (idx, &sw_color) in swatches.iter().enumerate() {
            let tip = format!(
                "Swatch #{} ({}) — Click to edit",
                idx + 1,
                sw_color.to_hex()
            );
            if paint_editor_swatch(ui, sw_color, idx == active_slot, t)
                .on_hover_text(tip)
                .clicked()
            {
                clicked_slot = Some(idx);
            }
        }
        if swatches.len() < MAX_PALETTE_COLORS
            && paint_add_swatch_cell(ui, t)
                .on_hover_text("Add swatch")
                .clicked()
        {
            add_slot_clicked = true;
        }
    });

    if let Some(idx) = clicked_slot {
        active_slot = idx;
        next_confirm = false;
        let sw_color = swatches[idx];
        app.view.color_picker.sync_from_color(sw_color);
        apply_color_to_target(app, sw_color);
    } else if add_slot_clicked {
        next_confirm = false;
        let next_color =
            next_distinct_swatch_color(app.view.color_picker.current_color(), &swatches);
        if app
            .session
            .execute(
                "palette.setColor",
                &json!({
                    "preset_id": preset_id,
                    "target": palette_target_str(target),
                    "color_hex": next_color.to_hex(),
                    "apply_to_pen": false,
                }),
            )
            .is_ok()
            && let Some(updated) = app.session.prefs.palette_by_id(&preset_id)
        {
            let new_len = updated.colors(target).len();
            if new_len > 0 {
                active_slot = new_len - 1;
                let added_color = updated.colors(target)[active_slot];
                app.view.color_picker.sync_from_color(added_color);
                apply_color_to_target(app, added_color);
            }
        }
    } else if remove_slot_clicked
        && can_remove
        && app
            .session
            .execute(
                "palette.removeColor",
                &json!({
                    "preset_id": preset_id,
                    "target": palette_target_str(target),
                    "index": active_slot,
                }),
            )
            .is_ok()
        && let Some(updated) = app.session.prefs.palette_by_id(&preset_id)
    {
        let remaining = updated.colors(target);
        if !remaining.is_empty() {
            active_slot = active_slot.min(remaining.len() - 1);
            let next_color = remaining[active_slot];
            app.view.color_picker.sync_from_color(next_color);
            apply_color_to_target(app, next_color);
        }
    }

    ui.add_space(6.0);
    let mut color_changed = draw_sv_square(ui, &mut app.view.color_picker, SV_BOX_HEIGHT_EDITOR, t);
    ui.add_space(6.0);
    color_changed |= draw_hue_bar(ui, &mut app.view.color_picker, t);
    ui.add_space(8.0);
    color_changed |= draw_preview_and_hex_row(ui, &mut app.view.color_picker, t);

    if color_changed {
        next_confirm = false;
        let picked = app.view.color_picker.current_color();
        let _ = app.session.execute(
            "palette.setColor",
            &json!({
                "preset_id": preset_id,
                "target": palette_target_str(target),
                "index": active_slot,
                "color": picked,
                "apply_to_pen": false,
            }),
        );
        apply_color_to_target(app, picked);
    }

    ui.add_space(8.0);
    let (div_rect, _) = ui.allocate_exact_size(Vec2::new(POPOVER_WIDTH, 1.0), Sense::hover());
    if ui.is_rect_visible(div_rect) {
        ui.painter().line_segment(
            [
                pos2(div_rect.left(), div_rect.center().y),
                pos2(div_rect.right(), div_rect.center().y),
            ],
            Stroke::new(1.0, t.border),
        );
    }
    ui.add_space(6.0);

    let mut deleted = false;
    ui.horizontal(|ui| {
        if !next_confirm {
            if draw_danger_button(ui, Some(Icon::Trash), "Delete Palette", false, true, t)
                .on_hover_text("Delete this custom palette")
                .clicked()
            {
                next_confirm = true;
            }
        } else {
            ui.label(
                RichText::new("Delete palette?")
                    .size(11.5)
                    .strong()
                    .color(t.danger_fg),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, Icon::Close, Some("Cancel"), false, true, t).clicked() {
                    next_confirm = false;
                }
                if draw_danger_button(ui, Some(Icon::Trash), "Confirm", true, true, t).clicked()
                    && app
                        .session
                        .execute("palette.deletePreset", &json!({ "id": preset_id }))
                        .is_ok()
                {
                    deleted = true;
                }
            });
        }
    });

    if deleted {
        app.view.color_dock.selected_wildcard = None;
        app.view.color_picker.clear_name_draft();
        app.view.color_picker.mode = ColorPopoverMode::Palettes;
        if let Some(&first) = app.session.prefs.palette(target).first() {
            app.view.color_picker.sync_from_color(first);
            apply_color_to_target(app, first);
        }
    } else {
        app.view.color_picker.mode = ColorPopoverMode::PaletteEditor {
            preset_id,
            selected_slot: active_slot,
            confirm_delete: next_confirm,
        };
    }
}

pub(crate) fn commit_palette_name_draft(app: &mut DrawCraftApp) {
    let ColorPopoverMode::PaletteEditor { ref preset_id, .. } = app.view.color_picker.mode else {
        return;
    };
    let preset_id = preset_id.clone();
    let Some(existing) = app.session.prefs.palette_by_id(&preset_id).cloned() else {
        return;
    };
    if existing.built_in {
        return;
    }
    let trimmed = app.view.color_picker.preset_name_draft.trim().to_string();
    if trimmed.is_empty() || !trimmed.chars().any(|c| !c.is_control()) {
        app.view.color_picker.preset_name_draft = existing.name;
        return;
    }
    if trimmed != existing.name {
        let _ = app.session.execute(
            "palette.savePreset",
            &json!({
                "id": preset_id,
                "name": trimmed,
            }),
        );
        if let Some(updated) = app.session.prefs.palette_by_id(&preset_id) {
            app.view.color_picker.preset_name_draft = updated.name.clone();
        }
    }
}

fn next_distinct_swatch_color(current: Color, existing: &[Color]) -> Color {
    if !existing.iter().any(|&c| same_rgb(c, current)) {
        return current.with_alpha(1.0);
    }
    let [mut h, mut s, mut v] = current.with_alpha(1.0).to_hsv();
    s = s.max(0.80);
    v = v.max(0.90);
    for _ in 0..8 {
        h = (h + 0.14).rem_euclid(1.0);
        let candidate = Color::from_hsv(h, s, v, 1.0);
        if !existing.iter().any(|&c| same_rgb(c, candidate)) {
            return candidate;
        }
    }
    current.with_alpha(1.0)
}

fn next_unique_palette_name(presets: &[ColorPalettePreset], base: &str) -> String {
    if !presets.iter().any(|p| p.name.eq_ignore_ascii_case(base)) {
        return base.to_string();
    }
    let mut n = 2usize;
    loop {
        let candidate = format!("{base} {n}");
        if !presets
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

/// Paints an interactive saturation/value square for normalized `hsv` (`[0..=1; 3]`).
pub(crate) fn paint_sv_square_picker(
    ui: &mut egui::Ui,
    size: Vec2,
    hsv: &mut [f32; 3],
    t: &Tokens,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let hue = hsv[0].rem_euclid(1.0);

    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let steps = 16usize;
        let mut mesh = Mesh::default();
        for iy in 0..=steps {
            let fy = iy as f32 / steps as f32;
            let v = 1.0 - fy;
            for ix in 0..=steps {
                let fx = ix as f32 / steps as f32;
                let s = fx;
                let c = to_color32(Color::from_hsv(hue, s, v, 1.0));
                mesh.vertices.push(Vertex {
                    pos: pos2(
                        egui::lerp(rect.left()..=rect.right(), fx),
                        egui::lerp(rect.top()..=rect.bottom(), fy),
                    ),
                    uv: WHITE_UV,
                    color: c,
                });
            }
        }
        let stride = steps + 1;
        for iy in 0..steps {
            for ix in 0..steps {
                let a = (iy * stride + ix) as u32;
                let b = a + 1;
                let c = ((iy + 1) * stride + ix) as u32;
                let d = c + 1;
                mesh.add_triangle(a, b, d);
                mesh.add_triangle(a, d, c);
            }
        }
        p.add(egui::Shape::mesh(mesh));
        p.rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );

        let current = Color::from_hsv(hue, hsv[1], hsv[2], 1.0);
        let handle_pos = pos2(
            egui::lerp(rect.left()..=rect.right(), hsv[1].clamp(0.0, 1.0)),
            egui::lerp(rect.top()..=rect.bottom(), 1.0 - hsv[2].clamp(0.0, 1.0)),
        );
        let ring_color = if hsv[2] < 0.55 {
            Color32::WHITE
        } else {
            Color32::from_rgb(0x14, 0x19, 0x23)
        };
        p.circle_filled(handle_pos, 5.5, to_color32(current));
        p.circle_stroke(handle_pos, 6.0, Stroke::new(1.8, ring_color));
    }

    if (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        hsv[1] = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
        hsv[2] = (1.0 - (pos.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
        return true;
    }
    false
}

/// Paints an interactive rainbow hue slider (`0.0..=1.0`), horizontal or vertical.
pub(crate) fn paint_hue_bar_picker(
    ui: &mut egui::Ui,
    size: Vec2,
    vertical: bool,
    hue_01: &mut f32,
    t: &Tokens,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        paint_hue_bar_visuals(ui.painter(), rect, vertical, *hue_01, t);
    }

    if (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let frac = if vertical {
            (pos.y - rect.top()) / rect.height().max(1.0)
        } else {
            (pos.x - rect.left()) / rect.width().max(1.0)
        };
        *hue_01 = frac.clamp(0.0, 1.0);
        return true;
    }
    false
}

fn paint_hue_bar_visuals(p: &egui::Painter, rect: Rect, vertical: bool, hue_01: f32, t: &Tokens) {
    let stops = [
        (0.0_f32, Color32::from_rgb(255, 0, 0)),
        (1.0 / 6.0, Color32::from_rgb(255, 255, 0)),
        (2.0 / 6.0, Color32::from_rgb(0, 255, 0)),
        (3.0 / 6.0, Color32::from_rgb(0, 255, 255)),
        (4.0 / 6.0, Color32::from_rgb(0, 0, 255)),
        (5.0 / 6.0, Color32::from_rgb(255, 0, 255)),
        (1.0, Color32::from_rgb(255, 0, 0)),
    ];
    let mut mesh = Mesh::default();
    for (idx, &(frac, col)) in stops.iter().enumerate() {
        let (p0, p1) = if vertical {
            let y = egui::lerp(rect.top()..=rect.bottom(), frac);
            (pos2(rect.left(), y), pos2(rect.right(), y))
        } else {
            let x = egui::lerp(rect.left()..=rect.right(), frac);
            (pos2(x, rect.top()), pos2(x, rect.bottom()))
        };
        mesh.vertices.push(Vertex {
            pos: p0,
            uv: WHITE_UV,
            color: col,
        });
        mesh.vertices.push(Vertex {
            pos: p1,
            uv: WHITE_UV,
            color: col,
        });
        if idx > 0 {
            let base = (idx as u32 - 1) * 2;
            mesh.add_triangle(base, base + 2, base + 3);
            mesh.add_triangle(base, base + 3, base + 1);
        }
    }
    p.add(egui::Shape::mesh(mesh));
    p.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );

    let clamped_h = hue_01.clamp(0.0, 1.0);
    let marker = if vertical {
        let hue_y = egui::lerp(rect.top()..=rect.bottom(), clamped_h);
        Rect::from_center_size(
            pos2(rect.center().x, hue_y),
            Vec2::new(rect.width() + 2.0, 6.0),
        )
    } else {
        let hue_x = egui::lerp(rect.left()..=rect.right(), clamped_h);
        Rect::from_center_size(
            pos2(hue_x, rect.center().y),
            Vec2::new(6.0, rect.height() + 2.0),
        )
    };
    p.rect_filled(marker, CornerRadius::same(2), Color32::WHITE);
    p.rect_stroke(
        marker,
        CornerRadius::same(2),
        Stroke::new(1.2, t.text),
        StrokeKind::Outside,
    );
}

fn draw_sv_square(
    ui: &mut egui::Ui,
    state: &mut ColorPickerState,
    height: f32,
    t: &Tokens,
) -> bool {
    if paint_sv_square_picker(ui, Vec2::new(POPOVER_WIDTH, height), &mut state.hsv, t) {
        state.hex_draft = state.current_color().to_hex();
        return true;
    }
    false
}

fn draw_hue_bar(ui: &mut egui::Ui, state: &mut ColorPickerState, t: &Tokens) -> bool {
    if paint_hue_bar_picker(
        ui,
        Vec2::new(POPOVER_WIDTH, HUE_BAR_HEIGHT),
        false,
        &mut state.hsv[0],
        t,
    ) {
        // Boost low saturation/brightness so picking a hue from Black or Grey gives a vivid color.
        let was_dark = state.hsv[2] < 0.35;
        if was_dark || state.hsv[1] < 0.35 {
            state.hsv[1] = state.hsv[1].max(0.88);
        }
        if state.hsv[2] < 0.45 {
            state.hsv[2] = 0.95;
        }
        state.hex_draft = state.current_color().to_hex();
        return true;
    }
    false
}

fn draw_preview_and_hex_row(ui: &mut egui::Ui, state: &mut ColorPickerState, t: &Tokens) -> bool {
    let mut changed = false;
    let current = state.current_color();
    let [r, g, b, _] = current.to_rgba8();

    ui.horizontal(|ui| {
        ui.set_height(26.0);
        let (preview_rect, _) = ui.allocate_exact_size(Vec2::new(34.0, 26.0), Sense::hover());
        ui.painter()
            .rect_filled(preview_rect, CornerRadius::same(5), to_color32(current));
        ui.painter().rect_stroke(
            preview_rect,
            CornerRadius::same(5),
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );

        crate::theme::studio_row_label(
            ui,
            RichText::new("Hex").size(11.5).color(t.text_muted),
            26.0,
        );
        let hex_resp = ui.add_sized(
            [76.0, 26.0],
            egui::TextEdit::singleline(&mut state.hex_draft)
                .desired_width(72.0)
                .char_limit(7)
                .font(egui::TextStyle::Monospace)
                .margin(egui::Margin::symmetric(6, 2))
                .align(egui::Align2::LEFT_CENTER),
        );
        if hex_resp.changed()
            && let Some(parsed) = Color::from_hex(&state.hex_draft)
        {
            state.hsv = parsed.with_alpha(1.0).to_hsv();
            changed = true;
        }
        if hex_resp.lost_focus() {
            state.hex_draft = state.current_color().to_hex();
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::theme::studio_row_label(
                ui,
                RichText::new(format!("{r},{g},{b}"))
                    .size(10.5)
                    .monospace()
                    .color(t.text_muted),
                26.0,
            );
        });
    });
    changed
}

fn paint_editor_swatch(
    ui: &mut egui::Ui,
    color: Color,
    selected: bool,
    t: &Tokens,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let center = rect.center();
        let hov_t =
            crate::motion::hover_factor(ui.ctx(), resp.id.with("hov"), resp.hovered() && !selected);
        if selected {
            p.circle_stroke(center, 9.5, Stroke::new(1.8, t.accent));
        } else if hov_t > 0.01 {
            p.circle_stroke(
                center,
                8.8,
                Stroke::new(1.1, t.text_muted.gamma_multiply(0.5 * hov_t)),
            );
        }
        p.circle_filled(center, 7.2, to_color32(color.with_alpha(1.0)));
        let rim = if t.is_dark {
            Color32::from_white_alpha(48)
        } else {
            Color32::from_black_alpha(36)
        };
        p.circle_stroke(center, 7.2, Stroke::new(0.9, rim));
    }
    resp
}

fn paint_add_swatch_cell(ui: &mut egui::Ui, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hov_t = crate::motion::hover_factor(ui.ctx(), resp.id.with("hov"), resp.hovered());
        let fill = crate::motion::lerp_color(t.panel_bg, t.border.gamma_multiply(0.45), hov_t);
        let stroke_c = crate::motion::lerp_color(t.border, t.text_muted, hov_t);
        let icon_c = crate::motion::lerp_color(t.text_muted, t.text, hov_t);
        p.circle_filled(rect.center(), 8.5, fill);
        p.circle_stroke(rect.center(), 8.5, Stroke::new(1.0, stroke_c));
        paint_icon(
            p,
            Rect::from_center_size(rect.center(), Vec2::splat(11.0)),
            Icon::Plus,
            icon_c,
        );
    }
    resp
}

fn draw_danger_button(
    ui: &mut egui::Ui,
    icon: Option<Icon>,
    label: &str,
    filled: bool,
    enabled: bool,
    t: &Tokens,
) -> egui::Response {
    let font = egui::FontId::proportional(11.8);
    let fg = if !enabled {
        t.text_muted.gamma_multiply(0.55)
    } else {
        t.danger_fg
    };
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let icon_w = if icon.is_some() { 14.0 + 5.0 } else { 0.0 };
    let width = (10.0 + icon_w + galley.size().x + 10.0).max(28.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), sense);
    if ui.is_rect_visible(rect) {
        let hov_t = if enabled {
            crate::motion::hover_factor(ui.ctx(), resp.id.with("hov"), resp.hovered())
        } else {
            0.0
        };
        let fill = if !enabled {
            t.toolbar_bg
        } else if filled {
            crate::motion::lerp_color(t.danger_bg, t.danger_border.gamma_multiply(0.45), hov_t)
        } else {
            crate::motion::lerp_color(t.panel_bg, t.danger_bg, hov_t)
        };
        let border = if !enabled {
            t.border
        } else if filled || hov_t > 0.05 {
            t.danger_border
        } else {
            t.border
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(7), fill);
        p.rect_stroke(
            rect,
            CornerRadius::same(7),
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        let mut cursor_x = rect.left() + 10.0;
        if let Some(ic) = icon {
            let ic_rect =
                Rect::from_center_size(pos2(cursor_x + 7.0, rect.center().y), Vec2::splat(14.0));
            paint_icon(p, ic_rect, ic, fg);
            cursor_x += 19.0;
        }
        p.galley(
            pos2(cursor_x, rect.center().y - galley.size().y * 0.5),
            galley,
            fg,
        );
    }
    resp
}

fn apply_color_to_target(app: &mut DrawCraftApp, color: Color) {
    if app.view.text_insert_mode || app.view.text_editing {
        crate::toolbar::apply_draft_text_color(app, color);
        return;
    }
    if app.view.shape_insert_mode {
        let active_kind = app.session.active_pen_kind();
        if PaletteTarget::for_pen(active_kind) == app.view.color_picker.target {
            let _ = app.session.execute(
                "pen.setSettings",
                &json!({
                    "kind": active_kind,
                    "color": color,
                }),
            );
        }
        return;
    }
    if app.session.active_tool == ToolKind::Select {
        if app.view.selected_objects.len() > 1 {
            let targets = crate::toolbar::selected_targets_json(&app.view.selected_objects);
            let _ = app.session.execute(
                "selection.update",
                &json!({ "targets": targets, "color": color }),
            );
            return;
        }
        let Some(selected) = app.view.selected_object else {
            return;
        };
        let _ = match selected {
            crate::SelectedObject::Stroke(id) => app
                .session
                .execute("stroke.update", &json!({ "id": id.0, "color": color })),
            crate::SelectedObject::Shape(index) => app
                .session
                .execute("shape.update", &json!({ "index": index, "color": color })),
            crate::SelectedObject::Text(index) => app
                .session
                .execute("text.update", &json!({ "index": index, "color": color })),
            crate::SelectedObject::Image(_) => Ok(serde_json::Value::Null),
        };
        return;
    }
    let ToolKind::Pen(active_kind) = app.session.active_tool else {
        return;
    };
    if PaletteTarget::for_pen(active_kind) != app.view.color_picker.target {
        return;
    }
    let _ = app.session.execute(
        "pen.setSettings",
        &json!({
            "kind": active_kind,
            "color": color,
        }),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn color_picker_renders_popover_updates_palette_and_closes_on_escape() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);

        let created = app
            .session
            .execute(
                "palette.savePreset",
                &json!({
                    "name": "Editable Group",
                    "copy_from_active": true,
                    "select": true,
                }),
            )
            .unwrap();
        let preset_id = created["id"].as_str().unwrap().to_string();

        let custom = Color::from_hex("#3366CC").unwrap();
        let anchor = Rect::from_min_size(pos2(500.0, 12.0), Vec2::splat(24.0));
        app.view.color_picker.open_edit_swatch(
            PaletteTarget::Ink,
            preset_id.clone(),
            2,
            custom,
            anchor,
        );
        assert!(app.view.color_picker.open);
        assert_eq!(app.view.color_picker.hex_draft, "#3366CC");

        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| {
            crate::theme::apply_light_visuals(ui.ctx());
            show_popover(&mut app, ui);
        });
        assert!(!out.shapes.is_empty());
        out.drop_without_applying_deltas();

        // Update palette slot #2 via engine command just as EditSwatch mode does.
        let edited = Color::from_hex("#CC4422").unwrap();
        app.view.color_picker.sync_from_color(edited);
        app.session
            .execute(
                "palette.setColor",
                &json!({
                    "preset_id": preset_id,
                    "target": "ink",
                    "index": 2,
                    "color_hex": app.view.color_picker.current_color().to_hex(),
                    "apply_to_pen": true,
                }),
            )
            .unwrap();
        assert_eq!(app.session.prefs.ink_palette[2].to_hex(), "#CC4422");
        assert_eq!(app.session.active_pen().color.to_hex(), "#CC4422");

        // Pressing Escape dismisses the popover cleanly.
        let esc_raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1280.0, 800.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let out = ctx.run_ui(esc_raw, |ui| {
            crate::theme::apply_light_visuals(ui.ctx());
            show_popover(&mut app, ui);
        });
        out.drop_without_applying_deltas();
        assert!(!app.view.color_picker.open);
    }

    #[test]
    fn color_picker_renders_three_defaults_and_supports_create_rename_and_delete() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);

        let anchor = Rect::from_min_size(pos2(500.0, 12.0), Vec2::splat(24.0));
        let ink = app.session.active_pen().color;
        app.view
            .color_picker
            .open_palettes(PenKind::FountainPen, ink, anchor);

        let ctx = egui::Context::default();
        let mut time = 0.0_f64;
        let mut shapes = Vec::new();
        for _ in 0..4 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        let texts = collect_picker_texts(&shapes);
        for expected in [
            "Built-in",
            "My Palettes",
            "Classic",
            "Pastel",
            "Earth Tones",
            "New Palette",
        ] {
            assert!(
                texts.contains(expected),
                "expected {expected:?} in Palettes mode: {texts}"
            );
        }

        // Create a new custom preset and render switched state cleanly.
        let created = app
            .session
            .execute(
                "palette.savePreset",
                &json!({ "name": "Studio Neon", "seed_color_hex": "#FF3366", "select": true }),
            )
            .unwrap();
        let id = created["id"].as_str().unwrap().to_string();
        assert_eq!(app.session.prefs.palette_presets.len(), 4);
        assert_eq!(app.session.prefs.ink_palette.len(), 1);

        let mut shapes2 = Vec::new();
        for _ in 0..2 {
            time += 0.05;
            shapes2 = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        let texts2 = collect_picker_texts(&shapes2);
        assert!(texts2.contains("Studio Neon"), "got: {texts2}");

        // Edit (rename) custom preset and delete it cleanly while built-in 3 remain.
        app.session
            .execute(
                "palette.savePreset",
                &json!({ "id": id, "name": "Renamed Group" }),
            )
            .unwrap();
        assert_eq!(
            app.session.prefs.active_palette().unwrap().name,
            "Renamed Group"
        );
        app.session
            .execute("palette.deletePreset", &json!({ "id": id }))
            .unwrap();
        assert_eq!(app.session.prefs.palette_presets.len(), 3);
    }

    fn find_picker_text_center(
        shapes: &[egui::epaint::ClippedShape],
        needle: &str,
    ) -> Option<egui::Pos2> {
        fn visit(shape: &egui::epaint::Shape, needle: &str) -> Option<egui::Pos2> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text().contains(needle) => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                egui::epaint::Shape::Vec(children) => {
                    children.iter().find_map(|child| visit(child, needle))
                }
                _ => None,
            }
        }
        shapes.iter().find_map(|c| visit(&c.shape, needle))
    }

    fn collect_picker_texts(shapes: &[egui::epaint::ClippedShape]) -> String {
        fn visit(shape: &egui::epaint::Shape, out: &mut String) {
            match shape {
                egui::epaint::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::epaint::Shape::Vec(children) => {
                    for child in children {
                        visit(child, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for c in shapes {
            visit(&c.shape, &mut out);
        }
        out
    }

    #[test]
    fn preview_and_hex_row_aligns_label_hex_input_and_rgb() {
        let ctx = egui::Context::default();
        let t = Tokens::light();
        let mut state = ColorPickerState {
            hsv: Color::from_hex("#2563EB")
                .unwrap_or(Color::BLACK)
                .with_alpha(1.0)
                .to_hsv(),
            hex_draft: "#2563EB".to_string(),
            ..Default::default()
        };
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(400.0, 200.0),
            )),
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| {
            crate::theme::apply_light_visuals(ui.ctx());
            let _ = draw_preview_and_hex_row(ui, &mut state, &t);
        });
        let shapes = out.shapes.clone();
        out.drop_without_applying_deltas();

        let ys: Vec<f32> = ["Hex", "#2563EB", "37,99,235"]
            .iter()
            .map(|needle| {
                find_picker_text_center(&shapes, needle)
                    .map(|p| p.y)
                    .unwrap_or_else(|| panic!("expected color picker row text {needle:?}"))
            })
            .collect();
        let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
        let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max_y - min_y <= 1.5,
            "Hex label, hex input, and RGB readout should align vertically: {ys:?}"
        );
    }

    fn step_popover_frame(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        time: f64,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1280.0, 800.0),
                )),
                time: Some(time),
                predicted_dt: 0.04,
                events,
                ..Default::default()
            },
            |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                show_popover(app, ui);
            },
        );
        let shapes = out.shapes.clone();
        out.drop_without_applying_deltas();
        shapes
    }

    fn click_popover_point(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        time: &mut f64,
        pos: egui::Pos2,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut shapes = Vec::new();
        for pressed in [true, false] {
            *time += 0.04;
            shapes = step_popover_frame(
                ctx,
                app,
                *time,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        shapes
    }

    #[test]
    fn dragging_hue_bar_from_black_boosts_saturation_and_value_to_vivid() {
        let ctx = egui::Context::default();
        let t = Tokens::light();
        let black = drawcraft_color::INK_PALETTE[0].color;
        let mut state = ColorPickerState::default();
        state.sync_from_color(black);

        let screen = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(400.0, 120.0));
        let mut hue_rect = Rect::NOTHING;
        let mut time = 0.0_f64;
        for _ in 0..3 {
            time += 0.05;
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    predicted_dt: 0.05,
                    ..Default::default()
                },
                |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    egui::Area::new(ui.make_persistent_id("test_hue_area"))
                        .fade_in(false)
                        .fixed_pos(pos2(10.0, 10.0))
                        .show(ui.ctx(), |ui| {
                            let _ = draw_hue_bar(ui, &mut state, &t);
                            hue_rect = ui.min_rect();
                        });
                },
            );
            out.drop_without_applying_deltas();
        }

        // Click near cyan-green center of the hue bar.
        let click_pos = pos2(
            hue_rect.left() + hue_rect.width() * 0.55,
            hue_rect.center().y,
        );
        let mut changed = false;
        for pressed in [true, false] {
            time += 0.04;
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    predicted_dt: 0.04,
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
                },
                |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    egui::Area::new(ui.make_persistent_id("test_hue_area"))
                        .fade_in(false)
                        .fixed_pos(pos2(10.0, 10.0))
                        .show(ui.ctx(), |ui| {
                            changed |= draw_hue_bar(ui, &mut state, &t);
                        });
                },
            );
            out.drop_without_applying_deltas();
        }
        assert!(changed);
        assert!(
            state.hsv[1] >= 0.80 && state.hsv[2] >= 0.90,
            "picking hue from Ink Black should yield vivid SV, got {:?}",
            state.hsv
        );
    }

    #[test]
    fn wildcard_picker_shows_only_color_picker_and_persists_without_mutating_presets() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);
        let original_classic = app.session.prefs.ink_palette.clone();
        let initial_hl_wildcard = app.session.prefs.wildcard_highlighter;

        let anchor = Rect::from_min_size(pos2(500.0, 12.0), Vec2::splat(24.0));
        let black = drawcraft_color::INK_PALETTE[0].color;
        app.view
            .color_picker
            .open_wildcard(PenKind::FountainPen, black, anchor);

        let ctx = egui::Context::default();
        let mut time = 0.0_f64;
        let mut shapes = Vec::new();
        for _ in 0..4 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        let texts = collect_picker_texts(&shapes);
        assert!(texts.contains("Custom Color"));
        for forbidden in [
            "Built-in",
            "My Palettes",
            "Classic",
            "Pastel",
            "New Palette",
        ] {
            assert!(
                !texts.contains(forbidden),
                "wildcard mode must not render palette UI ({forbidden}): {texts}"
            );
        }

        let sv_rect = shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Mesh(m) => {
                    let mut r = Rect::NOTHING;
                    for v in &m.vertices {
                        r.extend_with(v.pos);
                    }
                    if r.height() >= 100.0 { Some(r) } else { None }
                }
                _ => None,
            })
            .next()
            .expect("SV square mesh in color picker");

        let vivid_pos = pos2(sv_rect.right() - 12.0, sv_rect.top() + 12.0);
        let _ = click_popover_point(&ctx, &mut app, &mut time, vivid_pos);

        let picked = app.session.active_pen().color;
        assert_ne!(picked, black);
        assert_eq!(app.session.prefs.ink_palette, original_classic);
        assert_eq!(app.session.prefs.wildcard_ink, picked);
        assert_eq!(app.session.prefs.wildcard_highlighter, initial_hl_wildcard);
    }

    #[test]
    fn clicking_preset_chip_selects_preset_and_applies_first_color() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);

        let anchor = Rect::from_min_size(pos2(500.0, 12.0), Vec2::splat(24.0));
        let black = drawcraft_color::INK_PALETTE[0].color;
        app.view
            .color_picker
            .open_palettes(PenKind::FountainPen, black, anchor);

        let ctx = egui::Context::default();
        let mut time = 0.0_f64;
        let mut shapes = Vec::new();
        for _ in 0..4 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        let pastel_pos =
            find_picker_text_center(&shapes, "Pastel").expect("Pastel palette row in picker");
        let _ = click_popover_point(&ctx, &mut app, &mut time, pastel_pos);

        assert_eq!(
            app.session.prefs.active_palette_id,
            drawcraft_doc::BUILTIN_PALETTE_PASTEL_ID
        );
        assert_eq!(
            app.session.active_pen().color,
            drawcraft_color::PASTEL_INK_PALETTE[0].color
        );
    }

    #[test]
    fn palettes_new_button_enters_editor_adds_swatch_renames_and_deletes_with_confirmation() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);

        let anchor = Rect::from_min_size(pos2(500.0, 12.0), Vec2::splat(24.0));
        let ink = app.session.active_pen().color;
        app.view
            .color_picker
            .open_palettes(PenKind::FountainPen, ink, anchor);

        let ctx = egui::Context::default();
        let mut time = 0.0_f64;
        let mut shapes = Vec::new();
        for _ in 0..4 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }

        let new_btn = find_picker_text_center(&shapes, "New Palette").expect("New Palette button");
        let _ = click_popover_point(&ctx, &mut app, &mut time, new_btn);
        assert!(matches!(
            app.view.color_picker.mode,
            ColorPopoverMode::PaletteEditor { .. }
        ));
        assert_eq!(app.session.prefs.palette_presets.len(), 4);
        let created_id = app.session.prefs.active_palette_id.clone();

        // Step frames so PaletteEditor renders cleanly.
        for _ in 0..3 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        assert!(collect_picker_texts(&shapes).contains("Ink Swatches (1/12)"));

        // Click the trailing '+' swatch cell to add a second swatch.
        fn find_circle_center_with_radius(
            shapes: &[egui::epaint::ClippedShape],
            target_r: f32,
        ) -> Option<egui::Pos2> {
            fn visit(shape: &egui::epaint::Shape, target_r: f32) -> Option<egui::Pos2> {
                match shape {
                    egui::epaint::Shape::Circle(c) if (c.radius - target_r).abs() < 0.05 => {
                        Some(c.center)
                    }
                    egui::epaint::Shape::Vec(children) => {
                        children.iter().find_map(|child| visit(child, target_r))
                    }
                    _ => None,
                }
            }
            shapes.iter().find_map(|c| visit(&c.shape, target_r))
        }
        let add_cell_pos = find_circle_center_with_radius(&shapes, 8.5)
            .expect("Add swatch circle cell in PaletteEditor");
        let _ = click_popover_point(&ctx, &mut app, &mut time, add_cell_pos);
        for _ in 0..2 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        assert_eq!(app.session.prefs.ink_palette.len(), 2);
        assert!(collect_picker_texts(&shapes).contains("Ink Swatches (2/12)"));

        // Rename via draft + losing focus (commit_palette_name_draft).
        app.view.color_picker.preset_name_draft = "Chemistry".to_string();
        commit_palette_name_draft(&mut app);
        assert_eq!(
            app.session.prefs.palette_by_id(&created_id).unwrap().name,
            "Chemistry"
        );

        // Click Delete Palette -> enters confirmation; Escape cancels confirmation without closing.
        let del_btn =
            find_picker_text_center(&shapes, "Delete Palette").expect("Delete Palette button");
        let _ = click_popover_point(&ctx, &mut app, &mut time, del_btn);
        assert!(matches!(
            app.view.color_picker.mode,
            ColorPopoverMode::PaletteEditor {
                confirm_delete: true,
                ..
            }
        ));
        for _ in 0..2 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        assert!(collect_picker_texts(&shapes).contains("Delete palette?"));

        time += 0.04;
        let _ = step_popover_frame(
            &ctx,
            &mut app,
            time,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(app.view.color_picker.open);
        assert!(matches!(
            app.view.color_picker.mode,
            ColorPopoverMode::PaletteEditor {
                confirm_delete: false,
                ..
            }
        ));
        for _ in 0..2 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }

        // Click Delete Palette -> Confirm deletes custom palette and returns to Palettes mode.
        let del_btn =
            find_picker_text_center(&shapes, "Delete Palette").expect("Delete Palette button");
        let _ = click_popover_point(&ctx, &mut app, &mut time, del_btn);
        for _ in 0..2 {
            time += 0.05;
            shapes = step_popover_frame(&ctx, &mut app, time, Vec::new());
        }
        let confirm_btn = find_picker_text_center(&shapes, "Confirm").expect("Confirm button");
        let _ = click_popover_point(&ctx, &mut app, &mut time, confirm_btn);

        assert!(matches!(
            app.view.color_picker.mode,
            ColorPopoverMode::Palettes
        ));
        assert_eq!(app.session.prefs.palette_presets.len(), 3);
        assert!(app.session.prefs.palette_by_id(&created_id).is_none());
    }
}
