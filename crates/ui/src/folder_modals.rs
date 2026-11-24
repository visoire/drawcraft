//! New Folder, Rename Folder, Folder Color, and Delete Folder dialogs for the Notebooks Library.
//!
//! All four use the shared centered modal architecture in [`crate::modal`]:
//! focused inputs, a visual location list, and a preset + custom color picker.

use drawcraft_color::Color;
use drawcraft_doc::FOLDER_COLOR_PRESETS;
use egui::{Color32, CornerRadius, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2};
use serde_json::json;

use crate::DrawCraftApp;
use crate::color_picker::{paint_hue_bar_picker, paint_sv_square_picker};
use crate::icons::{
    FOLDER_ICON_CHOICES, Icon, SwitchLayout, SwitchOption, compact_icon_button, paint_icon,
    segmented_switch,
};
use crate::library_folders::{
    LibraryScope, current_scope, display_folder_path, folder_leaf, folder_parent, folder_tree,
    folder_visual_colors, known_library_folders, move_notebook_to_folder, paint_studio_folder_icon,
    path_within, plural, set_scope, validate_folder_name,
};
use crate::modal::{
    self, ActiveModal, danger_action_button, primary_action_button, secondary_action_button,
};
use crate::theme::{Tokens, to_color32};

/// 3 rows × 9 curated folder colors  (deep, vibrant, pastel),
/// with the 27th slot reserved for the custom HSV/hex color picker toggle.
const FOLDER_SWATCH_ROWS: &[&[(&str, &str)]] = &[
    &[
        ("Deep Crimson", "#8B1D1D"),
        ("Burnt Sienna", "#9A3412"),
        ("Bronze", "#92400E"),
        ("Forest Green", "#166534"),
        ("Royal Blue", "#1D4ED8"),
        ("Deep Purple", "#6B21A8"),
        ("Plum", "#86198F"),
        ("Charcoal", "#52525B"),
        ("Graphite", "#18181B"),
    ],
    &[
        ("Coral Red", "#F87171"),
        ("Tangerine", "#FB923C"),
        ("Marigold", "#FACC15"),
        ("Emerald", "#4ADE80"),
        ("Sky Blue", "#63B3FD"),
        ("Lavender", "#C084FC"),
        ("Rose Pink", "#F472B6"),
        ("Warm Gray", "#A8A29E"),
        ("Pure White", "#F4F4F5"),
    ],
    &[
        ("Soft Blush", "#FCA5A5"),
        ("Peach", "#FDBA74"),
        ("Butter", "#FDE047"),
        ("Mint", "#86EFAC"),
        ("Ice Blue", "#BFDBFE"),
        ("Lilac", "#E9D5FF"),
        ("Bubblegum", "#FBCFE8"),
        ("Sand", "#E7E5E4"),
    ],
];

/// Interactive draft state for picking a custom RGB folder color.
#[derive(Clone, Debug)]
pub struct FolderColorDraftState {
    pub custom_open: bool,
    /// `[hue (0..=1), saturation (0..=1), value (0..=1)]`.
    pub hsv: [f32; 3],
    pub hex_draft: String,
}

impl Default for FolderColorDraftState {
    fn default() -> Self {
        let initial = Color::from_hex("#2563EB").unwrap_or(Color::BLACK);
        Self {
            custom_open: false,
            hsv: initial.to_hsv(),
            hex_draft: initial.to_hex(),
        }
    }
}

impl FolderColorDraftState {
    /// Synchronizes draft state from `color`, opening custom controls when a non-preset color is set.
    pub fn sync_from_color(&mut self, color: Option<Color>) {
        match color {
            Some(c) => {
                self.set_color(c);
                self.custom_open = !is_preset_folder_color(c);
            }
            None => {
                let fallback = Color::from_hex("#2563EB").unwrap_or(Color::BLACK);
                self.set_color(fallback);
                self.custom_open = false;
            }
        }
    }

    pub fn set_color(&mut self, color: Color) {
        let opaque = color.sanitized().with_alpha(1.0);
        self.hsv = opaque.to_hsv();
        self.hex_draft = opaque.to_hex();
    }

    pub fn current_color(&self) -> Color {
        Color::from_hsv(self.hsv[0], self.hsv[1], self.hsv[2], 1.0)
    }
}

/// Cell edge (px) and column count shared by the folder color grid and the folder icon grid.
const FOLDER_GRID_CELL: f32 = 37.0;
const FOLDER_GRID_COLS: usize = 9;

fn same_rgb(a: Color, b: Color) -> bool {
    a.to_rgba8()[..3] == b.to_rgba8()[..3]
}

fn is_preset_folder_color(color: Color) -> bool {
    FOLDER_COLOR_PRESETS
        .iter()
        .filter_map(|&(_, hex)| Color::from_hex(hex))
        .any(|preset| same_rgb(color, preset))
}

/// Opens the New Folder dialog inside `parent` (`""` = top level). When
/// `move_notebook` is set, that notebook moves into the folder once it exists.
pub(crate) fn open_new_folder(app: &mut DrawCraftApp, parent: &str, move_notebook: Option<usize>) {
    app.view.folder_dialog_target.clear();
    app.view.folder_dialog_name.clear();
    app.view.folder_dialog_parent = parent.to_string();
    app.view.folder_dialog_move_notebook = move_notebook;
    app.view.folder_dialog_focus_pending = true;
    app.view.folder_dialog_color = None;
    app.view.folder_dialog_icon = None;
    app.view.folder_dialog_tab = 0;
    app.view.folder_color_draft.sync_from_color(None);
    modal::open(app, ActiveModal::NewFolder);
}

/// Opens the Edit Folder dialog for `path`.
pub(crate) fn open_rename_folder(app: &mut DrawCraftApp, path: &str) {
    let existing_color = app.session.prefs.folder_color(path);
    let existing_icon = app.session.prefs.folder_icon(path).map(str::to_string);
    app.view.folder_dialog_target = path.to_string();
    app.view.folder_dialog_name = folder_leaf(path).to_string();
    app.view.folder_dialog_parent = folder_parent(path).to_string();
    app.view.folder_dialog_move_notebook = None;
    app.view.folder_dialog_focus_pending = true;
    app.view.folder_dialog_color = existing_color;
    app.view.folder_dialog_icon = existing_icon;
    app.view.folder_dialog_tab = 0;
    app.view.folder_color_draft.sync_from_color(existing_color);
    modal::open(app, ActiveModal::RenameFolder);
}

/// Opens the Folder Color dialog for `path`.
pub(crate) fn open_folder_color(app: &mut DrawCraftApp, path: &str) {
    let existing_color = app.session.prefs.folder_color(path);
    app.view.folder_dialog_target = path.to_string();
    app.view.folder_dialog_move_notebook = None;
    app.view.folder_dialog_focus_pending = false;
    app.view.folder_dialog_color = existing_color;
    app.view.folder_color_draft.sync_from_color(existing_color);
    modal::open(app, ActiveModal::FolderColor);
}

/// Opens the Tag Color dialog for `tag`.
pub(crate) fn open_tag_color(app: &mut DrawCraftApp, tag: &str) {
    let clean = drawcraft_doc::sanitize_notebook_tag(tag).unwrap_or_else(|| tag.to_string());
    let existing_color = app.session.prefs.tag_color(&clean);
    app.view.tag_dialog_target = clean;
    app.view.tag_dialog_color = existing_color;
    app.view.tag_color_draft.sync_from_color(existing_color);
    modal::open(app, ActiveModal::TagColor);
}

/// Opens the Delete Folder confirmation for `path`.
pub(crate) fn open_delete_folder(app: &mut DrawCraftApp, path: &str) {
    app.view.folder_dialog_target = path.to_string();
    app.view.folder_dialog_move_notebook = None;
    app.view.folder_delete_all_pending = false;
    modal::open(app, ActiveModal::DeleteFolder);
}

fn section_title(ui: &mut egui::Ui, title: &str, subtitle: &str, t: &Tokens) {
    ui.label(RichText::new(title).size(13.0).strong().color(t.text));
    ui.add(egui::Label::new(RichText::new(subtitle).size(11.0).color(t.text_muted)).wrap());
    ui.add_space(4.0);
}

/// Name input that grabs keyboard focus when the dialog opens. Returns `true` on Enter.
fn draw_name_field(ui: &mut egui::Ui, name: &mut String, focus_pending: &mut bool) -> bool {
    let id = ui.make_persistent_id("drawcraft_folder_dialog_name");
    let resp = ui.add_sized(
        [ui.available_width(), 34.0],
        egui::TextEdit::singleline(name)
            .id(id)
            .hint_text("Untitled")
            .char_limit(crate::library_folders::MAX_FOLDER_NAME_CHARS)
            .margin(Margin::symmetric(10, 4))
            .align(egui::Align2::CENTER_CENTER),
    );
    if *focus_pending {
        resp.request_focus();
        *focus_pending = false;
    }
    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// Shared studio-style folder studio:
/// large live folder preview, centered name input, and `[ Color | Icon ]` tabs.
fn draw_studio_folder_studio(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    validation: &Result<String, String>,
    t: &Tokens,
) -> bool {
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        let (preview_rect, _) = ui.allocate_exact_size(Vec2::new(118.0, 94.0), Sense::hover());
        paint_studio_folder_icon(
            ui.painter(),
            preview_rect,
            app.view.folder_dialog_color,
            app.view.folder_dialog_icon.as_deref(),
            t,
        );
        ui.add_space(14.0);
    });

    let enter = draw_name_field(
        ui,
        &mut app.view.folder_dialog_name,
        &mut app.view.folder_dialog_focus_pending,
    );
    if let Err(message) = validation
        && !app.view.folder_dialog_name.trim().is_empty()
    {
        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(message).size(11.0).color(t.danger_fg));
        });
    }

    ui.add_space(12.0);
    let tab_options = [
        SwitchOption::new(None, Some("Color"), "Pick a folder color", true),
        SwitchOption::new(None, Some("Icon"), "Pick a folder emblem icon", true),
    ];
    let (clicked_tab, _) = segmented_switch(
        ui,
        "studio_folder_modal_tabs",
        &tab_options,
        Some(app.view.folder_dialog_tab.min(1)),
        SwitchLayout::EqualWidth,
        t,
    );
    if let Some(next_tab) = clicked_tab {
        app.view.folder_dialog_tab = next_tab;
    }

    ui.add_space(14.0);
    if app.view.folder_dialog_tab.min(1) == 0 {
        draw_studio_color_grid(
            ui,
            &mut app.view.folder_dialog_color,
            &mut app.view.folder_color_draft,
            t,
        );
    } else {
        draw_studio_icon_grid(ui, &mut app.view.folder_dialog_icon, t);
    }
    enter
}

fn draw_studio_color_grid(
    ui: &mut egui::Ui,
    selected: &mut Option<Color>,
    draft: &mut FolderColorDraftState,
    t: &Tokens,
) {
    let default_sky = Color::from_hex("#63B3FD");
    let rim = if t.is_dark {
        Color32::from_white_alpha(44)
    } else {
        Color32::from_black_alpha(32)
    };
    // Same 9 x 37 px lattice as the icon grid, so switching Color <-> Icon never shifts anything.
    let cell = FOLDER_GRID_CELL;
    let gap = 0.0_f32;
    let row_w = FOLDER_GRID_COLS as f32 * cell;

    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (row_idx, row) in FOLDER_SWATCH_ROWS.iter().enumerate() {
            ui.allocate_ui_with_layout(
                Vec2::new(row_w, cell),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for &(name, hex) in *row {
                        let Some(swatch) = Color::from_hex(hex) else {
                            continue;
                        };
                        let is_default_sky = default_sky.is_some_and(|d| same_rgb(d, swatch));
                        let active = match *selected {
                            Some(c) => same_rgb(c, swatch),
                            None => is_default_sky,
                        };
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                        if ui.is_rect_visible(r) {
                            let p = ui.painter();
                            if active {
                                p.circle_stroke(r.center(), 13.5, Stroke::new(2.0, t.accent));
                            }
                            p.circle_filled(r.center(), 10.6, to_color32(swatch));
                            p.circle_stroke(r.center(), 10.6, Stroke::new(0.9, rim));
                            if active {
                                let lum = drawcraft_color::relative_luminance(swatch);
                                let check_c = if lum > 0.65 {
                                    Color32::from_rgb(0x1F, 0x29, 0x37)
                                } else {
                                    Color32::WHITE
                                };
                                paint_icon(
                                    p,
                                    Rect::from_center_size(r.center(), Vec2::splat(12.0)),
                                    Icon::Check,
                                    check_c,
                                );
                            }
                        }
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(name)
                            .clicked()
                        {
                            *selected = if is_default_sky { None } else { Some(swatch) };
                            draft.set_color(swatch);
                            draft.custom_open = false;
                        }
                    }

                    // 27th slot on row 3: Custom color wheel / HSV studio button.
                    if row_idx == 2 {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                        if ui.is_rect_visible(r) {
                            let p = ui.painter();
                            if draft.custom_open {
                                p.circle_stroke(r.center(), 13.5, Stroke::new(2.0, t.accent));
                            }
                            // Crisp 6-segment color wheel ring indicating custom color picker.
                            let wheel = [
                                Color32::from_rgb(0xEF, 0x44, 0x44),
                                Color32::from_rgb(0xF5, 0x9E, 0x0B),
                                Color32::from_rgb(0x22, 0xC5, 0x5E),
                                Color32::from_rgb(0x3B, 0x82, 0xF6),
                                Color32::from_rgb(0x8B, 0x5C, 0xF6),
                                Color32::from_rgb(0xEC, 0x48, 0x99),
                            ];
                            p.circle_filled(r.center(), 10.6, t.panel_bg);
                            for (i, c) in wheel.iter().enumerate() {
                                let a = (i as f32 / 6.0) * std::f32::consts::TAU;
                                let pt = pos2(
                                    r.center().x + a.cos() * 5.2,
                                    r.center().y + a.sin() * 5.2,
                                );
                                p.circle_filled(pt, 3.8, *c);
                            }
                            p.circle_stroke(r.center(), 10.6, Stroke::new(1.0, t.border));
                        }
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Custom color...")
                            .clicked()
                        {
                            draft.custom_open = !draft.custom_open;
                            if draft.custom_open && selected.is_none() {
                                *selected = Some(draft.current_color());
                            }
                        }
                    }
                },
            );
        }
    });

    let custom_id = ui.make_persistent_id("studio_folder_custom_color");
    crate::motion::animated_collapse(ui, custom_id, draft.custom_open, 178.0, |ui| {
        ui.add_space(10.0);
        draw_inline_color_studio(ui, "folder", selected, draft, t);
    });
}

fn draw_studio_icon_grid(ui: &mut egui::Ui, selected_icon: &mut Option<String>, t: &Tokens) {
    let cell = FOLDER_GRID_CELL;
    let gap = 0.0_f32;
    let cols = FOLDER_GRID_COLS;
    let row_w = cols as f32 * cell;
    let total = FOLDER_ICON_CHOICES.len() + 1; // icons + None slash cell

    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for row_start in (0..total).step_by(cols) {
            ui.allocate_ui_with_layout(
                Vec2::new(row_w, cell),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for idx in row_start..(row_start + cols).min(total) {
                        let (rect, resp) =
                            ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                        if let Some(&(key, label, icon)) = FOLDER_ICON_CHOICES.get(idx) {
                            let active = selected_icon
                                .as_deref()
                                .is_some_and(|k| k.eq_ignore_ascii_case(key));
                            if ui.is_rect_visible(rect) {
                                let p = ui.painter();
                                if active {
                                    p.rect_stroke(
                                        rect.shrink(3.0),
                                        CornerRadius::same(10),
                                        Stroke::new(2.0, t.accent),
                                        StrokeKind::Inside,
                                    );
                                } else if resp.hovered() {
                                    p.rect_filled(
                                        rect.shrink(3.0),
                                        CornerRadius::same(10),
                                        t.ghost_hover,
                                    );
                                }
                                paint_icon(
                                    p,
                                    Rect::from_center_size(rect.center(), Vec2::splat(20.0)),
                                    icon,
                                    if active { t.accent } else { t.text },
                                );
                            }
                            if resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(label)
                                .clicked()
                            {
                                *selected_icon = Some(key.to_string());
                            }
                        } else {
                            // 21st cell: Clear / No icon.
                            let active = selected_icon.is_none();
                            if ui.is_rect_visible(rect) {
                                let p = ui.painter();
                                if active {
                                    p.rect_stroke(
                                        rect.shrink(3.0),
                                        CornerRadius::same(10),
                                        Stroke::new(2.0, t.accent),
                                        StrokeKind::Inside,
                                    );
                                } else if resp.hovered() {
                                    p.rect_filled(
                                        rect.shrink(3.0),
                                        CornerRadius::same(10),
                                        t.ghost_hover,
                                    );
                                }
                                let d = 8.0_f32;
                                p.line_segment(
                                    [
                                        pos2(rect.center().x - d, rect.center().y - d),
                                        pos2(rect.center().x + d, rect.center().y + d),
                                    ],
                                    Stroke::new(1.8, t.danger_fg),
                                );
                            }
                            if resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("No icon")
                                .clicked()
                            {
                                *selected_icon = None;
                            }
                        }
                    }
                },
            );
        }
    });
}

fn draw_folder_color_section(
    ui: &mut egui::Ui,
    selected: &mut Option<Color>,
    draft: &mut FolderColorDraftState,
    t: &Tokens,
) {
    draw_color_section(
        ui,
        "folder",
        "folder_color_studio_collapse",
        selected,
        draft,
        t,
    );
}

fn draw_color_section(
    ui: &mut egui::Ui,
    target_noun: &str,
    collapse_salt: &str,
    selected: &mut Option<Color>,
    draft: &mut FolderColorDraftState,
    t: &Tokens,
) {
    section_title(
        ui,
        "Color",
        "Pick a preset color or craft any custom shade.",
        t,
    );
    let is_custom = selected.is_some_and(|c| !is_preset_folder_color(c));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        if compact_icon_button(
            ui,
            Icon::Reset,
            Some("Default"),
            selected.is_none(),
            true,
            t,
        )
        .on_hover_text(format!("Use the default theme {target_noun} color"))
        .clicked()
        {
            *selected = None;
            draft.custom_open = false;
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
            let active = selected.is_some_and(|c| same_rgb(c, swatch));
            let (dot_rect, dot_resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
            if ui.is_rect_visible(dot_rect) {
                let p = ui.painter();
                if active {
                    p.circle_stroke(dot_rect.center(), 9.8, Stroke::new(1.8, t.accent));
                }
                p.circle_filled(dot_rect.center(), 7.6, to_color32(swatch));
                p.circle_stroke(dot_rect.center(), 7.6, Stroke::new(0.9, rim));
            }
            if dot_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("{name} ({hex})"))
                .clicked()
            {
                *selected = Some(swatch);
                draft.set_color(swatch);
                draft.custom_open = false;
            }
        }
        if compact_icon_button(
            ui,
            Icon::Palette,
            Some("Custom"),
            is_custom || draft.custom_open,
            true,
            t,
        )
        .on_hover_text("Pick any custom color using HSV sliders or a #RRGGBB hex code")
        .clicked()
        {
            draft.custom_open = !draft.custom_open;
            if draft.custom_open && selected.is_none() {
                *selected = Some(draft.current_color());
            }
        }
    });

    let custom_id = ui.make_persistent_id(collapse_salt);
    crate::motion::animated_collapse(ui, custom_id, draft.custom_open, 178.0, |ui| {
        ui.add_space(8.0);
        draw_inline_color_studio(ui, target_noun, selected, draft, t);
    });
}

fn draw_inline_color_studio(
    ui: &mut egui::Ui,
    target_noun: &str,
    selected: &mut Option<Color>,
    draft: &mut FolderColorDraftState,
    t: &Tokens,
) {
    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let sv_w = (ui.available_width() - 26.0).max(120.0);
                    let mut changed =
                        paint_sv_square_picker(ui, Vec2::new(sv_w, 72.0), &mut draft.hsv, t);
                    changed |=
                        paint_hue_bar_picker(ui, Vec2::new(16.0, 72.0), true, &mut draft.hsv[0], t);
                    if changed {
                        let c = draft.current_color();
                        draft.hex_draft = c.to_hex();
                        *selected = Some(c);
                    }
                });
                draw_color_hex_row(ui, target_noun, selected, draft, t);
            });
        });
}

fn draw_color_hex_row(
    ui: &mut egui::Ui,
    target_noun: &str,
    selected: &mut Option<Color>,
    draft: &mut FolderColorDraftState,
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
                .margin(Margin::symmetric(6, 2))
                .align(egui::Align2::LEFT_CENTER),
        );
        if hex_resp.changed()
            && let Some(parsed) = Color::from_hex(draft.hex_draft.trim())
        {
            let opaque = parsed.with_alpha(1.0);
            draft.hsv = opaque.to_hsv();
            *selected = Some(opaque);
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
                .on_hover_text(format!(
                    "Collapse custom color controls while keeping this {target_noun} color"
                ))
                .clicked()
            {
                draft.custom_open = false;
            }
        });
    });
}

// ---------------------------------------------------------------------------
// New Folder
// ---------------------------------------------------------------------------

fn new_folder_validation(app: &DrawCraftApp) -> Result<String, String> {
    validate_folder_name(
        &app.view.folder_dialog_name,
        &app.view.folder_dialog_parent,
        &known_library_folders(app),
        None,
    )
}

/// Renders the New Folder dialog body. Returns `true` when the modal should close.
pub(crate) fn render_new_folder_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let result = new_folder_validation(app);
    let enter = draw_studio_folder_studio(app, ui, &result, t);

    if let Some(title) = app
        .view
        .folder_dialog_move_notebook
        .and_then(|idx| app.session.notebooks.get(idx))
        .map(|doc| doc.title.clone())
    {
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(format!("\"{title}\" will be moved into the new folder."))
                    .size(11.5)
                    .color(t.accent),
            );
        });
    }
    enter && result.is_ok() && submit_new_folder(app)
}

/// Renders the New Folder dialog footer. Returns `true` when the modal should close.
pub(crate) fn render_new_folder_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let valid = new_folder_validation(app).is_ok();
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_action_button(ui, "Done", valid, t).clicked() && submit_new_folder(app) {
                close = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                close = true;
            }
        });
    });
    close
}

fn submit_new_folder(app: &mut DrawCraftApp) -> bool {
    let Ok(path) = new_folder_validation(app) else {
        return false;
    };
    let mut payload = json!({ "path": path });
    if let Some(color) = app.view.folder_dialog_color {
        payload["color"] = json!(color);
    }
    if let Some(ref icon) = app.view.folder_dialog_icon {
        payload["icon"] = json!(icon);
    }
    if app
        .session
        .execute("library.folder.create", &payload)
        .is_err()
    {
        return false;
    }
    if let Some(index) = app.view.folder_dialog_move_notebook.take() {
        let _ = move_notebook_to_folder(app, index, &path);
    }
    set_scope(app, LibraryScope::Folder(path));
    app.view.folder_dialog_name.clear();
    true
}

// ---------------------------------------------------------------------------
// Edit Folder (Rename, Color & Icon)
// ---------------------------------------------------------------------------

fn rename_validation(app: &DrawCraftApp) -> Result<String, String> {
    let target = &app.view.folder_dialog_target;
    if target.is_empty() {
        return Err("This folder no longer exists.".to_string());
    }
    if path_within(&app.view.folder_dialog_parent, target) {
        return Err("A folder can't be moved inside itself.".to_string());
    }
    validate_folder_name(
        &app.view.folder_dialog_name,
        &app.view.folder_dialog_parent,
        &known_library_folders(app),
        Some(target),
    )
}

/// Renders the Edit Folder dialog body. Returns `true` when the modal should close.
pub(crate) fn render_rename_folder_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let result = rename_validation(app);
    let enter = draw_studio_folder_studio(app, ui, &result, t);
    enter && result.is_ok() && submit_rename_folder(app)
}

/// Renders the Edit Folder dialog footer. Returns `true` when the modal should close.
pub(crate) fn render_rename_folder_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let valid = rename_validation(app).is_ok();
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_action_button(ui, "Done", valid, t).clicked() && submit_rename_folder(app) {
                close = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                close = true;
            }
        });
    });
    close
}

fn remap_path(path: &str, old_root: &str, new_root: &str) -> Option<String> {
    if !path_within(path, old_root) {
        return None;
    }
    let suffix = path.get(old_root.len()..).unwrap_or("");
    Some(format!("{new_root}{suffix}"))
}

fn submit_rename_folder(app: &mut DrawCraftApp) -> bool {
    let Ok(new_path) = rename_validation(app) else {
        return false;
    };
    let old_path = app.view.folder_dialog_target.clone();
    let desired_color = app.view.folder_dialog_color;
    let existing_color = app.session.prefs.folder_color(&old_path);
    let color_changed = desired_color != existing_color;
    let desired_icon = app.view.folder_dialog_icon.clone();
    let existing_icon = app.session.prefs.folder_icon(&old_path).map(str::to_string);
    let icon_changed = desired_icon != existing_icon;

    if new_path == old_path && !color_changed && !icon_changed {
        return true;
    }
    let mut payload = json!({ "old_path": old_path, "new_path": new_path });
    if color_changed {
        match desired_color {
            Some(c) => payload["color"] = json!(c),
            None => payload["clear_color"] = json!(true),
        }
    }
    if icon_changed {
        match desired_icon {
            Some(icon) => payload["icon"] = json!(icon),
            None => payload["clear_icon"] = json!(true),
        }
    }
    if app
        .session
        .execute("library.folder.rename", &payload)
        .is_err()
    {
        return false;
    }
    if let LibraryScope::Folder(open) = current_scope(app)
        && let Some(moved) = remap_path(&open, &old_path, &new_path)
    {
        set_scope(app, LibraryScope::Folder(moved));
    }
    for collapsed in &mut app.view.library_collapsed_folders {
        if let Some(moved) = remap_path(collapsed, &old_path, &new_path) {
            *collapsed = moved;
        }
    }
    true
}

fn apply_folder_color_command(app: &mut DrawCraftApp, path: &str, color: Option<Color>) -> bool {
    let payload = match color {
        Some(c) => json!({ "path": path, "color": c }),
        None => json!({ "path": path, "clear": true }),
    };
    app.session
        .execute("library.folder.setColor", &payload)
        .is_ok()
}

fn apply_tag_color_command(app: &mut DrawCraftApp, tag: &str, color: Option<Color>) -> bool {
    let payload = match color {
        Some(c) => json!({ "tag": tag, "color": c }),
        None => json!({ "tag": tag, "clear": true }),
    };
    app.session
        .execute("library.tag.setColor", &payload)
        .is_ok()
}

fn draw_color_preview_card(
    ui: &mut egui::Ui,
    icon: Icon,
    title: &str,
    subtitle: &str,
    color: Option<Color>,
    show_color_pill: bool,
    t: &Tokens,
) {
    let vis = folder_visual_colors(color, t);
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
                    .rect_filled(badge, CornerRadius::same(9), vis.soft_fill);
                paint_icon(ui.painter(), badge.shrink(9.0), icon, vis.icon);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(14.0).strong().color(t.text));
                    ui.label(RichText::new(subtitle).size(11.0).color(t.text_muted));
                });
                if show_color_pill {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = color.map_or_else(|| "Default".to_string(), Color::to_hex);
                        let pill_fg = if color.is_some() {
                            vis.selected_text
                        } else {
                            t.text_muted
                        };
                        let pill_fill = if color.is_some() {
                            vis.soft_fill
                        } else {
                            t.field_bg
                        };
                        egui::Frame::NONE
                            .fill(pill_fill)
                            .stroke(Stroke::new(1.0, t.border))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.label(RichText::new(label).size(10.5).strong().color(pill_fg));
                            });
                    });
                }
            });
        });
}

fn draw_folder_preview_card(
    ui: &mut egui::Ui,
    path: &str,
    summary: &str,
    color: Option<Color>,
    show_color_pill: bool,
    t: &Tokens,
) {
    draw_color_preview_card(
        ui,
        Icon::Folder,
        folder_leaf(path),
        &format!("Library / {}  •  {summary}", display_folder_path(path)),
        color,
        show_color_pill,
        t,
    );
}

// ---------------------------------------------------------------------------
// Folder Color & Tag Color
// ---------------------------------------------------------------------------

/// Renders the Folder Color dialog body. Returns `true` when the modal should close.
pub(crate) fn render_folder_color_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let target = app.view.folder_dialog_target.clone();
    let Some(node) = folder_tree(app)
        .into_iter()
        .find(|node| node.path.eq_ignore_ascii_case(&target))
    else {
        ui.label(
            RichText::new("This folder no longer exists.")
                .size(12.0)
                .color(t.text_muted),
        );
        return false;
    };
    draw_folder_preview_card(
        ui,
        &node.path,
        &node.summary(),
        app.view.folder_dialog_color,
        true,
        t,
    );
    ui.add_space(12.0);
    draw_folder_color_section(
        ui,
        &mut app.view.folder_dialog_color,
        &mut app.view.folder_color_draft,
        t,
    );
    false
}

fn render_color_footer(
    ui: &mut egui::Ui,
    enabled: bool,
    t: &Tokens,
    save: impl FnOnce() -> bool,
) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_action_button(ui, "Save Color", enabled, t).clicked() && save() {
                close = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                close = true;
            }
        });
    });
    close
}

/// Renders the Folder Color dialog footer. Returns `true` when the modal should close.
pub(crate) fn render_folder_color_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let exists = !app.view.folder_dialog_target.is_empty();
    render_color_footer(ui, exists, t, || submit_folder_color(app))
}

fn submit_folder_color(app: &mut DrawCraftApp) -> bool {
    let target = app.view.folder_dialog_target.clone();
    if target.is_empty() {
        return false;
    }
    apply_folder_color_command(app, &target, app.view.folder_dialog_color)
}

/// Renders the Tag Color dialog body. Returns `true` when the modal should close.
pub(crate) fn render_tag_color_body(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let target = app.view.tag_dialog_target.clone();
    if target.is_empty() {
        ui.label(
            RichText::new("No tag selected.")
                .size(12.0)
                .color(t.text_muted),
        );
        return false;
    }
    let notebook_count = app
        .session
        .notebooks
        .iter()
        .filter(|doc| doc.tags.iter().any(|tag| tag.eq_ignore_ascii_case(&target)))
        .count();
    draw_color_preview_card(
        ui,
        Icon::Tag,
        &format!("#{target}"),
        &format!(
            "Tags  •  {}",
            plural(notebook_count, "notebook", "notebooks")
        ),
        app.view.tag_dialog_color,
        true,
        t,
    );
    ui.add_space(12.0);
    draw_color_section(
        ui,
        "tag",
        "tag_color_studio_collapse",
        &mut app.view.tag_dialog_color,
        &mut app.view.tag_color_draft,
        t,
    );
    false
}

/// Renders the Tag Color dialog footer. Returns `true` when the modal should close.
pub(crate) fn render_tag_color_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let exists = !app.view.tag_dialog_target.is_empty();
    render_color_footer(ui, exists, t, || submit_tag_color(app))
}

fn submit_tag_color(app: &mut DrawCraftApp) -> bool {
    let target = app.view.tag_dialog_target.clone();
    if target.is_empty() {
        return false;
    }
    apply_tag_color_command(app, &target, app.view.tag_dialog_color)
}

// ---------------------------------------------------------------------------
// Delete Folder
// ---------------------------------------------------------------------------

/// Renders the Delete Folder confirmation body.
pub(crate) fn render_delete_folder_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let target = app.view.folder_dialog_target.clone();
    let Some(node) = folder_tree(app)
        .into_iter()
        .find(|node| node.path.eq_ignore_ascii_case(&target))
    else {
        ui.label(
            RichText::new("This folder no longer exists.")
                .size(12.0)
                .color(t.text_muted),
        );
        return false;
    };
    let parent = folder_parent(&node.path);
    let destination = if parent.is_empty() {
        "Library".to_string()
    } else {
        display_folder_path(parent)
    };

    draw_folder_preview_card(ui, &node.path, &node.summary(), node.color, false, t);
    ui.add_space(10.0);

    if app.view.folder_delete_all_pending {
        // Second confirmation for the destructive variant.
        bullet(
            ui,
            Icon::AlertCircle,
            &format!(
                "Really delete \"{}\" and move {} to Trash? Notebooks stay in Trash for 30 days.",
                node.name(),
                plural(node.total_notebooks, "notebook", "notebooks")
            ),
            t,
        );
        return false;
    }

    let notebooks_line = if node.total_notebooks == 0 {
        "There are no notebooks inside, so nothing else changes.".to_string()
    } else {
        format!(
            "{} inside (including subfolders) will move to \"{destination}\". No notebook is deleted.",
            plural(node.total_notebooks, "notebook", "notebooks")
        )
    };
    bullet(ui, Icon::Notebook, &notebooks_line, t);
    if node.subfolders > 0 {
        let nested = folder_tree(app)
            .iter()
            .filter(|other| {
                path_within(&other.path, &node.path) && !other.path.eq_ignore_ascii_case(&node.path)
            })
            .count();
        bullet(
            ui,
            Icon::Folder,
            &format!(
                "{} inside it will be removed as well.",
                plural(nested, "subfolder", "subfolders")
            ),
            t,
        );
    }
    false
}

fn bullet(ui: &mut egui::Ui, icon: Icon, text: &str, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
        paint_icon(ui.painter(), rect, icon, t.text_muted);
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(t.text)).wrap());
    });
}

/// Renders the Delete Folder confirmation footer.
pub(crate) fn render_delete_folder_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let target = app.view.folder_dialog_target.clone();
    let exists = !target.is_empty();
    let notebooks_inside = folder_tree(app)
        .iter()
        .find(|node| node.path.eq_ignore_ascii_case(&target))
        .map_or(0, |node| node.total_notebooks);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.view.folder_delete_all_pending {
                if danger_action_button(ui, "Yes, delete everything", exists, t).clicked()
                    && submit_delete_folder(app, true)
                {
                    close = true;
                }
                if secondary_action_button(ui, "Back", true, t).clicked() {
                    app.view.folder_delete_all_pending = false;
                }
                return;
            }
            if danger_action_button(ui, "Delete Folder", exists, t).clicked()
                && submit_delete_folder(app, false)
            {
                close = true;
            }
            if notebooks_inside > 0
                && secondary_action_button(ui, "Delete Folder and Notebooks...", exists, t)
                    .clicked()
            {
                app.view.folder_delete_all_pending = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                close = true;
            }
        });
    });
    close
}
fn submit_delete_folder(app: &mut DrawCraftApp, with_notebooks: bool) -> bool {
    let target = app.view.folder_dialog_target.clone();
    if target.is_empty() {
        return false;
    }
    let parent = folder_parent(&target).to_string();
    if with_notebooks {
        // Highest index first so earlier indexes stay valid while notebooks move to Trash.
        let doomed: Vec<usize> = app
            .session
            .notebooks
            .iter()
            .enumerate()
            .filter(|(_, doc)| path_within(&doc.folder_path, &target))
            .map(|(idx, _)| idx)
            .collect();
        for idx in doomed.into_iter().rev() {
            let _ = app
                .session
                .execute("notebook.delete", &json!({ "index": idx }));
        }
        crate::sync_view_to_active_notebook(app);
    }
    if app
        .session
        .execute(
            "library.folder.delete",
            &json!({ "path": target, "move_notebooks_to": parent }),
        )
        .is_err()
    {
        return false;
    }
    if let LibraryScope::Folder(open) = current_scope(app)
        && path_within(&open, &target)
    {
        let next = if parent.is_empty() {
            LibraryScope::All
        } else {
            LibraryScope::Folder(parent)
        };
        set_scope(app, next);
    }
    app.view
        .library_collapsed_folders
        .retain(|collapsed| !path_within(collapsed, &target));
    app.view.folder_dialog_target.clear();
    app.view.folder_delete_all_pending = false;
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_folder_dialog_creates_nested_folder_moves_notebook_and_opens_it() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute("library.folder.create", &json!({ "path": "School" }))
            .unwrap();
        open_new_folder(&mut app, "School", Some(0));
        assert_eq!(app.view.active_modal, Some(ActiveModal::NewFolder));
        assert!(new_folder_validation(&app).is_err());

        app.view.folder_dialog_name = "school".to_string();
        app.view.folder_dialog_parent.clear();
        assert!(
            new_folder_validation(&app).is_err(),
            "duplicate at top level"
        );

        app.view.folder_dialog_parent = "School".to_string();
        app.view.folder_dialog_name = " Math ".to_string();
        assert!(submit_new_folder(&mut app));
        assert!(
            app.session
                .prefs
                .library_folders
                .contains(&"School/Math".to_string())
        );
        assert_eq!(app.session.notebooks[0].folder_path, "School/Math");
        assert_eq!(
            current_scope(&app),
            LibraryScope::Folder("School/Math".to_string())
        );
    }

    #[test]
    fn rename_dialog_moves_folder_and_keeps_open_view_in_sync() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "notebook.create",
                &json!({ "title": "Calc", "folder_path": "School/Math" }),
            )
            .unwrap();
        set_scope(&mut app, LibraryScope::Folder("School/Math".to_string()));
        open_rename_folder(&mut app, "School");
        assert_eq!(app.view.folder_dialog_name, "School");
        assert_eq!(app.view.folder_dialog_parent, "");

        // A folder can never move into its own subtree.
        app.view.folder_dialog_parent = "School/Math".to_string();
        assert!(rename_validation(&app).is_err());

        app.view.folder_dialog_parent.clear();
        app.view.folder_dialog_name = "College".to_string();
        assert!(submit_rename_folder(&mut app));
        assert_eq!(app.session.notebooks[1].folder_path, "College/Math");
        assert_eq!(
            current_scope(&app),
            LibraryScope::Folder("College/Math".to_string())
        );
    }

    #[test]
    fn delete_dialog_moves_notebooks_to_parent_and_leaves_deleted_view() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "notebook.create",
                &json!({ "title": "Calc", "folder_path": "School/Math/Calc" }),
            )
            .unwrap();
        set_scope(
            &mut app,
            LibraryScope::Folder("School/Math/Calc".to_string()),
        );
        open_delete_folder(&mut app, "School/Math");
        assert_eq!(app.view.active_modal, Some(ActiveModal::DeleteFolder));
        assert!(submit_delete_folder(&mut app, false));
        assert_eq!(app.session.notebooks[1].folder_path, "School");
        assert_eq!(
            current_scope(&app),
            LibraryScope::Folder("School".to_string())
        );
        assert!(
            !app.session
                .prefs
                .library_folders
                .iter()
                .any(|f| f.starts_with("School/Math"))
        );
    }

    #[test]
    fn delete_with_notebooks_trashes_them_and_favorites_follow_renames() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "notebook.create",
                &json!({ "title": "Calc", "folder_path": "School/Math" }),
            )
            .unwrap();
        let before = app.session.notebooks.len();
        app.session
            .execute(
                "library.folder.setFavorite",
                &json!({ "path": "School", "favorite": true }),
            )
            .unwrap();
        assert!(app.session.prefs.is_folder_favorite("school"));
        app.session
            .execute(
                "library.folder.rename",
                &json!({ "old_path": "School", "new_path": "College" }),
            )
            .unwrap();
        assert!(app.session.prefs.is_folder_favorite("College"));
        assert!(!app.session.prefs.is_folder_favorite("School"));

        open_delete_folder(&mut app, "College");
        assert!(!app.view.folder_delete_all_pending);
        assert!(submit_delete_folder(&mut app, true));
        assert_eq!(app.session.notebooks.len(), before - 1);
        assert_eq!(app.session.trashed_notebooks.len(), 1);
        assert!(!app.session.prefs.is_folder_favorite("College"));
    }
    #[test]
    fn folder_dialogs_render_without_panicking() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "notebook.create",
                &json!({ "title": "Calc", "folder_path": "School/Math", "tags": ["exam"] }),
            )
            .unwrap();
        let t = Tokens::light();
        for open in [
            |app: &mut DrawCraftApp| open_new_folder(app, "School", None),
            |app: &mut DrawCraftApp| open_rename_folder(app, "School/Math"),
            |app: &mut DrawCraftApp| open_delete_folder(app, "School"),
            |app: &mut DrawCraftApp| {
                open_folder_color(app, "School/Math");
                app.view.folder_color_draft.custom_open = true;
            },
            |app: &mut DrawCraftApp| {
                open_tag_color(app, "exam");
                app.view.tag_color_draft.custom_open = true;
            },
        ] {
            open(&mut app);
            let ctx = egui::Context::default();
            for _ in 0..2 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    match app.view.active_modal {
                        Some(ActiveModal::NewFolder) => {
                            let _ = render_new_folder_body(&mut app, ui, &t);
                            let _ = render_new_folder_footer(&mut app, ui, &t);
                        }
                        Some(ActiveModal::RenameFolder) => {
                            let _ = render_rename_folder_body(&mut app, ui, &t);
                            let _ = render_rename_folder_footer(&mut app, ui, &t);
                        }
                        Some(ActiveModal::DeleteFolder) => {
                            let _ = render_delete_folder_body(&mut app, ui, &t);
                            let _ = render_delete_folder_footer(&mut app, ui, &t);
                        }
                        Some(ActiveModal::FolderColor) => {
                            let _ = render_folder_color_body(&mut app, ui, &t);
                            let _ = render_folder_color_footer(&mut app, ui, &t);
                        }
                        Some(ActiveModal::TagColor) => {
                            let _ = render_tag_color_body(&mut app, ui, &t);
                            let _ = render_tag_color_footer(&mut app, ui, &t);
                        }
                        _ => {}
                    }
                });
                out.drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn folder_color_workflows_persist_across_create_rename_and_color_dialog() {
        let mut app = DrawCraftApp::default();
        let teal = Color::from_hex("#0D9488").unwrap();
        let rose = Color::from_hex("#DB2777").unwrap();

        open_new_folder(&mut app, "", None);
        app.view.folder_dialog_name = "Biology".to_string();
        app.view.folder_dialog_color = Some(teal);
        assert!(submit_new_folder(&mut app));
        assert_eq!(app.session.prefs.folder_color("Biology"), Some(teal));

        // Rename dialog updates color even without changing name or parent.
        open_rename_folder(&mut app, "Biology");
        assert_eq!(app.view.folder_dialog_color, Some(teal));
        app.view.folder_dialog_color = Some(rose);
        assert!(submit_rename_folder(&mut app));
        assert_eq!(app.session.prefs.folder_color("Biology"), Some(rose));

        // Folder Color dialog sets and clears color cleanly.
        open_folder_color(&mut app, "Biology");
        assert_eq!(app.view.active_modal, Some(ActiveModal::FolderColor));
        assert_eq!(app.view.folder_dialog_color, Some(rose));
        app.view.folder_dialog_color = None;
        assert!(submit_folder_color(&mut app));
        assert_eq!(app.session.prefs.folder_color("Biology"), None);
    }

    #[test]
    fn tag_color_dialog_sets_updates_and_clears_color_cleanly() {
        let mut app = DrawCraftApp::default();
        let blue = Color::from_hex("#2563EB").unwrap();
        let amber = Color::from_hex("#D97706").unwrap();

        open_tag_color(&mut app, "#example");
        assert_eq!(app.view.active_modal, Some(ActiveModal::TagColor));
        assert_eq!(app.view.tag_dialog_target, "example");
        assert_eq!(app.view.tag_dialog_color, None);
        app.view.tag_dialog_color = Some(blue);
        assert!(submit_tag_color(&mut app));
        assert_eq!(app.session.prefs.tag_color("example"), Some(blue));

        open_tag_color(&mut app, "Example");
        assert_eq!(app.view.tag_dialog_color, Some(blue));
        app.view.tag_dialog_color = Some(amber);
        assert!(submit_tag_color(&mut app));
        assert_eq!(app.session.prefs.tag_color("EXAMPLE"), Some(amber));

        open_tag_color(&mut app, "example");
        app.view.tag_dialog_color = None;
        assert!(submit_tag_color(&mut app));
        assert_eq!(app.session.prefs.tag_color("example"), None);
    }
}
