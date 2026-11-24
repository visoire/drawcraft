//! Dedicated minimalist Settings drawer: Pen input stabilization with live
//! stroke preview, studio appearance, and live Telemetry.

use drawcraft_doc::{
    PenKind, PenSettings, StabilizationPreset, StrokeSample, StylusButtonAction,
    StylusButtonConfig, UserStudioConfig,
};
use drawcraft_geom::Affine2;
use drawcraft_paint::smooth_stroke_samples;
use egui::{CornerRadius, Frame, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2};
use serde_json::json;

use crate::icons::{
    Icon, SwitchLayout, SwitchOption, compact_icon_button, icon_button, paint_icon,
    section_card_header, segmented_switch,
};
use crate::theme::{Tokens, to_color32};
use crate::{DrawCraftApp, SettingsTab};

pub(crate) fn card_frame(t: &Tokens) -> Frame {
    Frame::new()
        .fill(t.field_bg)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .stroke(Stroke::new(1.0, t.border))
}

#[derive(Clone, PartialEq)]
struct LivePreviewMeshCache {
    pen: PenSettings,
    width_bits: u32,
    mesh: Option<std::sync::Arc<egui::Mesh>>,
}

fn build_live_preview_mesh(
    pen: &PenSettings,
    rect_width: f32,
) -> Option<std::sync::Arc<egui::Mesh>> {
    let count = 36usize;
    let x0 = 20.0_f64;
    let span = f64::from((rect_width - 40.0).max(20.0));
    let raw: Vec<StrokeSample> = (0..count)
        .map(|i| {
            let u = i as f64 / (count - 1) as f64;
            let wave = (u * std::f64::consts::TAU * 1.1).sin();
            // Small synthetic tremor showcases stabilizer behavior in the live preview.
            let tremor = ((i as f64) * 2.1).sin() * 2.2 * (1.0 - (2.0 * u - 1.0).powi(2));
            let pressure = 0.22 + 0.72 * ((u * std::f64::consts::PI).sin() as f32);
            let tilt_x = (wave * 24.0) as f32;
            StrokeSample {
                x: x0 + u * span,
                y: -wave * 15.0 + tremor,
                pressure,
                tilt_x,
                tilt_y: -8.0,
                rotation: 0.0,
                time_ms: i as f64 * 10.0,
            }
        })
        .collect();
    let smoothed = smooth_stroke_samples(&raw, &pen.smoothing, 1.0);
    crate::canvas::build_live_ribbon_mesh(pen, &smoothed, Affine2::IDENTITY, 0.85)
        .map(std::sync::Arc::new)
}

pub(crate) fn draw_live_stroke_preview(
    app: &DrawCraftApp,
    ui: &mut egui::Ui,
    kind: PenKind,
    t: &Tokens,
) {
    let pen = app.session.pen(kind);
    let theme = app.session.prefs.theme;
    let auto_contrast = app.session.prefs.auto_contrast_with_theme;
    let paper_colors = app
        .session
        .doc
        .paper_color
        .resolved_colors(theme, auto_contrast);
    let paper_bg = to_color32(paper_colors.background);
    let rule_c = to_color32(paper_colors.line);

    let desired = Vec2::new(ui.available_width(), 64.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let p = ui.painter_at(rect);

    p.rect_filled(rect, CornerRadius::same(7), paper_bg);
    p.line_segment(
        [
            pos2(rect.left() + 8.0, rect.center().y),
            pos2(rect.right() - 8.0, rect.center().y),
        ],
        Stroke::new(1.0, rule_c),
    );
    p.rect_stroke(
        rect,
        CornerRadius::same(7),
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );

    let mut display_pen = pen.clone();
    display_pen.color = app.session.doc.paper_color.resolve_ink_color(
        theme,
        auto_contrast,
        pen.color,
        matches!(pen.kind, PenKind::Highlighter),
    );
    let width_bits = rect.width().to_bits();
    let cache_id = ui.make_persistent_id("drawcraft_inspector_preview_cache");
    let cached = ui
        .ctx()
        .data(|d| d.get_temp::<LivePreviewMeshCache>(cache_id));
    let mesh_opt = match cached {
        Some(entry) if entry.pen == display_pen && entry.width_bits == width_bits => entry.mesh,
        _ => {
            let mesh = build_live_preview_mesh(&display_pen, rect.width());
            let entry = LivePreviewMeshCache {
                pen: display_pen,
                width_bits,
                mesh: mesh.clone(),
            };
            ui.ctx().data_mut(|d| d.insert_temp(cache_id, entry));
            mesh
        }
    };
    if let Some(local_mesh) = mesh_opt {
        let mut translated = (*local_mesh).clone();
        translated.translate(egui::vec2(rect.left(), rect.center().y));
        p.add(egui::Shape::mesh(translated));
    }

    let stab_tag = pen.smoothing.stabilization_summary();
    ui.add_space(3.0);
    ui.label(
        RichText::new(format!(
            "Preview: {} — {:.1} pt · {}",
            pen.kind.label(),
            pen.size,
            stab_tag
        ))
        .size(11.0)
        .color(t.text_muted),
    );
}

fn draw_smoothing_preset_card(
    ui: &mut egui::Ui,
    preset: StabilizationPreset,
    selected: bool,
    t: &Tokens,
) -> bool {
    let card_id = ui.make_persistent_id(("drawcraft_stab_preset_card", preset));
    let hover_mem_id = card_id.with("hov_mem");
    let prev_hovered = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(hover_mem_id))
        .unwrap_or(false);
    let select_t = crate::motion::hover_factor(ui.ctx(), card_id.with("sel"), selected);
    let hover_t =
        crate::motion::hover_factor(ui.ctx(), card_id.with("hov"), prev_hovered && !selected);

    let rest_fill = crate::motion::lerp_color(t.panel_bg, t.border.gamma_multiply(0.30), hover_t);
    let fill = crate::motion::lerp_color(rest_fill, t.accent_soft, select_t);
    let rest_border =
        crate::motion::lerp_color(t.border, t.text_muted.gamma_multiply(0.55), hover_t);
    let border_c = crate::motion::lerp_color(rest_border, t.accent, select_t);
    let stroke = Stroke::new(crate::motion::lerp_f32(1.0, 1.5, select_t), border_c);
    let title_c = crate::motion::lerp_color(t.text, t.accent, select_t);

    let badge_bg = if selected {
        t.accent.gamma_multiply(0.16)
    } else {
        t.field_bg
    };
    let badge_border = if selected {
        t.accent.gamma_multiply(0.45)
    } else {
        t.border
    };
    let badge_fg = if selected { t.accent } else { t.text_muted };

    let frame_resp = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if selected {
                    let (icon_r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    paint_icon(ui.painter(), icon_r, Icon::Check, t.accent);
                }
                ui.add(
                    egui::Label::new(
                        RichText::new(preset.label())
                            .size(12.2)
                            .strong()
                            .color(title_c),
                    )
                    .selectable(false),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    Frame::new()
                        .fill(badge_bg)
                        .stroke(Stroke::new(1.0, badge_border))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(preset.badge()).size(9.8).color(badge_fg),
                                )
                                .selectable(false),
                            );
                        });
                });
            });
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(
                    RichText::new(preset.description())
                        .size(11.0)
                        .color(t.text_muted),
                )
                .selectable(false)
                .wrap(),
            );
        });

    let resp = frame_resp
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(preset.description());
    ui.ctx()
        .data_mut(|d| d.insert_temp(hover_mem_id, resp.hovered()));
    resp.clicked()
}

pub(crate) fn draw_pen_smoothing_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    kind: PenKind,
    t: &Tokens,
) {
    card_frame(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_card_header(
            ui,
            Icon::Stabilizer,
            "Input stabilization",
            "Choose a curated stroke smoothing preset for the active pen.",
            t,
        );

        let active_preset = app.session.pen(kind).smoothing.preset();
        let mut chosen_preset = None;

        ui.add_space(2.0);
        for (idx, &preset) in StabilizationPreset::ALL.iter().enumerate() {
            if idx > 0 {
                ui.add_space(6.0);
            }
            if draw_smoothing_preset_card(ui, preset, preset == active_preset, t) {
                chosen_preset = Some(preset);
            }
        }

        if let Some(preset) = chosen_preset {
            let _ = app.session.execute(
                "pen.setSmoothing",
                &json!({
                    "kind": kind,
                    "preset": preset,
                }),
            );
        }
    });
}

struct StylusButtonRowSpec<'a> {
    id_salt: &'a str,
    title: &'a str,
    idle_badge: &'a str,
    active_badge: &'a str,
    active: bool,
    current: StylusButtonAction,
    options: &'a [StylusButtonAction],
}

fn draw_stylus_button_row(
    ui: &mut egui::Ui,
    spec: StylusButtonRowSpec<'_>,
    t: &Tokens,
) -> Option<StylusButtonAction> {
    let fill = if spec.active {
        t.accent_soft
    } else {
        t.panel_bg
    };
    let border = if spec.active { t.accent } else { t.border };
    let stroke_w = if spec.active { 1.5 } else { 1.0 };
    let title_color = if spec.active { t.accent } else { t.text };
    let badge_label = if spec.active {
        spec.active_badge
    } else {
        spec.idle_badge
    };
    let badge_bg = if spec.active {
        t.accent.gamma_multiply(0.16)
    } else {
        t.field_bg
    };
    let badge_border = if spec.active {
        t.accent.gamma_multiply(0.45)
    } else {
        t.border
    };
    let badge_fg = if spec.active { t.accent } else { t.text_muted };

    let mut chosen = None;
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(stroke_w, border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(spec.title)
                            .size(12.2)
                            .strong()
                            .color(title_color),
                    )
                    .selectable(false),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    Frame::new()
                        .fill(badge_bg)
                        .stroke(Stroke::new(1.0, badge_border))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(badge_label).size(9.8).color(badge_fg),
                                )
                                .selectable(false),
                            );
                        });
                });
            });
            ui.add_space(5.0);
            let combo_w = ui.available_width();
            crate::theme::studio_combo_box(ui, spec.id_salt, spec.current.label(), combo_w, |ui| {
                for &opt in spec.options {
                    if ui
                        .selectable_label(opt == spec.current, opt.label())
                        .on_hover_text(opt.description())
                        .clicked()
                    {
                        chosen = Some(opt);
                    }
                }
            });
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new(spec.current.description())
                        .size(11.0)
                        .color(t.text_muted),
                )
                .selectable(false)
                .wrap(),
            );
        });

    chosen.filter(|&next| next != spec.current)
}

pub(crate) fn draw_stylus_shortcuts_card(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let cfg = app.session.prefs.stylus_buttons.sanitized();
    let is_customized = cfg != StylusButtonConfig::default();
    let tel = app.stylus.telemetry;
    let device_label = app.stylus.connected_device_label();

    let mut reset_requested = false;
    let mut next_barrel = None;
    let mut next_secondary = None;
    let mut next_eraser = None;

    card_frame(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_card_header(
            ui,
            Icon::BallPen,
            "Stylus shortcuts",
            "Assign quick tool and edit actions to your connected stylus buttons.",
            t,
        );

        Frame::new()
            .fill(t.success_bg)
            .stroke(Stroke::new(1.0, t.success_border))
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::symmetric(9, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let reset_w = crate::icons::compact_icon_button_width(ui, "Reset");
                    let left_w =
                        (ui.available_width() - reset_w - ui.spacing().item_spacing.x).max(40.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_w, 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let (dot_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                            ui.painter()
                                .circle_filled(dot_rect.center(), 4.0, t.success_fg);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("Connected · {device_label}"))
                                        .size(11.5)
                                        .strong()
                                        .color(t.success_fg),
                                )
                                .truncate(),
                            );
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if compact_icon_button(
                            ui,
                            Icon::Reset,
                            Some("Reset"),
                            false,
                            is_customized,
                            t,
                        )
                        .on_hover_text("Restore default stylus button shortcuts")
                        .clicked()
                        {
                            reset_requested = true;
                        }
                    });
                });
            });

        ui.add_space(8.0);
        next_barrel = draw_stylus_button_row(
            ui,
            StylusButtonRowSpec {
                id_salt: "stylus_barrel_button_combo",
                title: "Side Button 1",
                idle_badge: "Primary Barrel",
                active_badge: "Pressed",
                active: tel.barrel_held,
                current: cfg.barrel_button,
                options: StylusButtonAction::ALL,
            },
            t,
        );

        ui.add_space(6.0);
        next_secondary = draw_stylus_button_row(
            ui,
            StylusButtonRowSpec {
                id_salt: "stylus_secondary_barrel_combo",
                title: "Side Button 2",
                idle_badge: "Secondary Barrel",
                active_badge: "Pressed",
                active: tel.secondary_barrel_held,
                current: cfg.secondary_barrel_button,
                options: StylusButtonAction::ALL,
            },
            t,
        );

        ui.add_space(6.0);
        next_eraser = draw_stylus_button_row(
            ui,
            StylusButtonRowSpec {
                id_salt: "stylus_eraser_end_combo",
                title: "Eraser End",
                idle_badge: "Inverted Tip",
                active_badge: "Active",
                active: tel.eraser_inverted,
                current: cfg.eraser_end,
                options: StylusButtonAction::ERASER_END_OPTIONS,
            },
            t,
        );
    });

    if reset_requested {
        let _ = app
            .session
            .execute("stylus.setButtons", &json!({ "reset": true }));
    } else if next_barrel.is_some() || next_secondary.is_some() || next_eraser.is_some() {
        let mut payload = serde_json::Map::new();
        if let Some(action) = next_barrel {
            payload.insert("barrel_button".to_string(), json!(action));
        }
        if let Some(action) = next_secondary {
            payload.insert("secondary_barrel_button".to_string(), json!(action));
        }
        if let Some(action) = next_eraser {
            payload.insert("eraser_end".to_string(), json!(action));
        }
        let _ = app
            .session
            .execute("stylus.setButtons", &serde_json::Value::Object(payload));
    }
}

fn draw_appearance_controls(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    switch_id_salt: &str,
    t: &Tokens,
) {
    let current = app.session.prefs.theme;
    let active_idx = drawcraft_doc::UiTheme::ALL
        .iter()
        .position(|&theme| theme == current);
    let options: Vec<SwitchOption<'_>> = drawcraft_doc::UiTheme::ALL
        .iter()
        .map(|&theme| {
            let icon = match theme {
                drawcraft_doc::UiTheme::Light => Icon::Sun,
                drawcraft_doc::UiTheme::Dark => Icon::Moon,
            };
            SwitchOption::new(Some(icon), Some(theme.label()), theme.description(), true)
        })
        .collect();
    let (clicked, _) = segmented_switch(
        ui,
        switch_id_salt,
        &options,
        active_idx,
        SwitchLayout::EqualWidth,
        t,
    );
    if let Some(idx) = clicked
        && let Some(&theme) = drawcraft_doc::UiTheme::ALL.get(idx)
    {
        let _ = app.session.execute("theme.set", &json!({ "theme": theme }));
    }

    ui.add_space(10.0);
    let mut auto_contrast = app.session.prefs.auto_contrast_with_theme;
    let check_resp = ui
        .checkbox(&mut auto_contrast, "Auto-adapt canvas colors to theme")
        .on_hover_text(
            "Invert paper polarity and rebalance ink contrast along its hue when switching themes. Saved files and exports keep their original colors.",
        );
    if check_resp.changed() {
        let _ = app.session.execute(
            "theme.set",
            &json!({ "auto_contrast_with_theme": auto_contrast }),
        );
    }
    ui.add(
        egui::Label::new(
            RichText::new(
                "Shifts paper light/dark to match the theme and adjusts ink brightness along its original hue for high contrast without changing exports.",
            )
            .size(11.0)
            .color(t.text_muted),
        )
        .wrap(),
    );
}

pub(crate) fn draw_appearance_card(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let current = app.session.prefs.theme;
    card_frame(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let header_icon = match current {
            drawcraft_doc::UiTheme::Light => Icon::Sun,
            drawcraft_doc::UiTheme::Dark => Icon::Moon,
        };
        section_card_header(
            ui,
            header_icon,
            "Appearance",
            "Switch the studio workspace between a bright daylight look and a low-glare dark interface.",
            t,
        );
        draw_appearance_controls(app, ui, "inspector_theme_switch", t);
    });
}

pub(crate) fn draw_app_scale_card(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let current_scale = UserStudioConfig::sanitize_ui_scale(app.session.prefs.ui_scale);
    let current_pct = (current_scale * 100.0).round() as i32;
    let is_default = (current_scale - UserStudioConfig::DEFAULT_UI_SCALE).abs() <= 0.005;
    let mut next_scale: Option<f32> = None;

    card_frame(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_card_header(
            ui,
            Icon::ZoomIn,
            "App Scale",
            "Resize interface panels, text, and controls. Also works anytime with Ctrl+Plus, Ctrl+Minus, and Ctrl+0.",
            t,
        );

        ui.horizontal(|ui| {
            ui.label(RichText::new("Current scale").size(12.0).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if compact_icon_button(ui, Icon::Reset, Some("Reset"), false, !is_default, t)
                    .on_hover_text("Reset app scale to 100% (Ctrl+0)")
                    .clicked()
                {
                    next_scale = Some(UserStudioConfig::DEFAULT_UI_SCALE);
                }

                let at_max = current_scale >= UserStudioConfig::MAX_UI_SCALE - 0.005;
                if compact_icon_button(ui, Icon::ZoomIn, None, false, !at_max, t)
                    .on_hover_text("Increase app scale by 10% (Ctrl+Plus)")
                    .clicked()
                {
                    next_scale = Some(((current_scale + 0.1) * 10.0).round() / 10.0);
                }

                Frame::new()
                    .fill(if is_default {
                        t.toolbar_bg
                    } else {
                        t.accent_soft
                    })
                    .stroke(Stroke::new(
                        1.0,
                        if is_default { t.border } else { t.accent },
                    ))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{current_pct}%"))
                                .size(12.0)
                                .strong()
                                .monospace()
                                .color(if is_default { t.text } else { t.accent }),
                        );
                    });

                let at_min = current_scale <= UserStudioConfig::MIN_UI_SCALE + 0.005;
                if compact_icon_button(ui, Icon::ZoomOut, None, false, !at_min, t)
                    .on_hover_text("Decrease app scale by 10% (Ctrl+Minus)")
                    .clicked()
                {
                    next_scale = Some(((current_scale - 0.1) * 10.0).round() / 10.0);
                }
            });
        });

        ui.add_space(8.0);
        let presets: &[(f32, &str)] = &[
            (0.80, "80%"),
            (1.00, "100%"),
            (1.25, "125%"),
            (1.50, "150%"),
            (1.75, "175%"),
        ];
        let active_preset = presets
            .iter()
            .position(|(val, _)| (current_scale - *val).abs() <= 0.005);
        let options: Vec<SwitchOption<'_>> = presets
            .iter()
            .map(|(val, label)| {
                let tip = if (*val - 1.0).abs() <= 0.005 {
                    "Default 100% interface scale (Ctrl+0)"
                } else {
                    "Set interface scale"
                };
                SwitchOption::new(None, Some(*label), tip, true)
            })
            .collect();
        let (clicked, _) = segmented_switch(
            ui,
            "inspector_ui_scale_presets",
            &options,
            active_preset,
            SwitchLayout::EqualWidth,
            t,
        );
        if let Some(idx) = clicked
            && let Some((val, _)) = presets.get(idx)
        {
            next_scale = Some(*val);
        }
    });

    if let Some(scale) = next_scale {
        let clean = UserStudioConfig::sanitize_ui_scale(scale);
        let _ = app
            .session
            .execute("theme.set", &json!({ "ui_scale": clean }));
    }
}

fn sanitize_metric(val: f32, lo: f32, hi: f32) -> f32 {
    if val.is_finite() {
        val.clamp(lo, hi)
    } else {
        0.0
    }
}

fn draw_metric_cell(ui: &mut egui::Ui, label: &str, value: &str, active: bool, t: &Tokens) {
    let fill = if active { t.accent_soft } else { t.panel_bg };
    let border = if active { t.accent } else { t.border };
    let value_color = if active { t.accent } else { t.text };
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(9, 6))
        .stroke(Stroke::new(1.0, border))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(label).size(10.5).color(t.text_muted));
            ui.label(
                RichText::new(value)
                    .size(12.5)
                    .strong()
                    .monospace()
                    .color(value_color),
            );
        });
}

pub(crate) fn draw_live_telemetry_card(app: &DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let tel = app.stylus.telemetry;
    let pressure = sanitize_metric(tel.pressure, 0.0, 1.0);
    let tilt_x = sanitize_metric(tel.tilt_x, -90.0, 90.0);
    let tilt_y = sanitize_metric(tel.tilt_y, -90.0, 90.0);
    let rotation = sanitize_metric(tel.rotation, 0.0, 360.0);
    let velocity = sanitize_metric(tel.velocity_px_ms, 0.0, 999.0);

    card_frame(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_card_header(
            ui,
            Icon::Telemetry,
            "Live Stylus Telemetry",
            "Monitor real-time digitizer pressure, tilt angles, barrel rotation, and stylus buttons.",
            t,
        );

        let (source_label, source_color, source_fill, source_border) = if tel.hardware_active {
            ("Hardware Stylus Active", t.accent, t.accent_soft, t.accent)
        } else {
            (
                "Mouse / Trackpad Fallback",
                t.text_muted,
                t.panel_bg,
                t.border,
            )
        };
        Frame::new()
            .fill(source_fill)
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::symmetric(9, 6))
            .stroke(Stroke::new(1.0, source_border))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter()
                        .circle_filled(dot_rect.center(), 4.0, source_color);
                    ui.label(RichText::new(source_label).size(12.0).strong().color(source_color));
                });
            });

        ui.add_space(8.0);
        let pressure_pct = (pressure * 100.0).round();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Pressure").size(11.5).strong().color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{pressure_pct:.0}% ({pressure:.2})"))
                        .size(11.5)
                        .monospace()
                        .color(t.text_muted),
                );
            });
        });
        ui.add_space(3.0);
        let (bar_rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(bar_rect, 5.0, t.border);
        let fill = Rect::from_min_size(
            bar_rect.min,
            Vec2::new(bar_rect.width() * pressure, bar_rect.height()),
        );
        p.rect_filled(fill, 5.0, t.accent);

        ui.add_space(8.0);
        ui.columns(2, |cols| {
            draw_metric_cell(&mut cols[0], "Tilt X", &format!("{tilt_x:+.1}°"), false, t);
            draw_metric_cell(&mut cols[1], "Tilt Y", &format!("{tilt_y:+.1}°"), false, t);
        });
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            draw_metric_cell(
                &mut cols[0],
                "Rotation",
                &format!("{rotation:.1}°"),
                false,
                t,
            );
            draw_metric_cell(
                &mut cols[1],
                "Speed",
                &format!("{velocity:.2} px/ms"),
                false,
                t,
            );
        });
        ui.add_space(6.0);
        ui.columns(2, |cols| {
            draw_metric_cell(
                &mut cols[0],
                "Eraser End",
                if tel.eraser_inverted { "ACTIVE" } else { "Off" },
                tel.eraser_inverted,
                t,
            );
            draw_metric_cell(
                &mut cols[1],
                "Barrel Button",
                if tel.barrel_held { "HELD" } else { "Off" },
                tel.barrel_held,
                t,
            );
        });
        ui.add_space(6.0);
        let stylus_connected = app.is_stylus_connected();
        ui.columns(2, |cols| {
            draw_metric_cell(
                &mut cols[0],
                "Side Button 2",
                if tel.secondary_barrel_held { "HELD" } else { "Off" },
                tel.secondary_barrel_held,
                t,
            );
            draw_metric_cell(
                &mut cols[1],
                "Stylus",
                if stylus_connected { "Connected" } else { "None" },
                stylus_connected,
                t,
            );
        });

        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Tip: Inverting a supported stylus or holding its side buttons runs your configured shortcuts. Customize button actions in the Pen tab while a stylus is connected.",
                )
                .size(11.0)
                .color(t.text_muted),
            )
            .wrap(),
        );
    });
}

pub(crate) fn draw_shortcuts_card(ui: &mut egui::Ui, t: &Tokens) {
    card_frame(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        section_card_header(
            ui,
            Icon::Info,
            "Keyboard & Canvas Shortcuts",
            "Quick keyboard and pointer shortcuts for switching tools and navigating pages.",
            t,
        );

        let shortcuts = [
            (
                "1 – 7",
                "Switch Fountain, Ball, Brush, Highlighter, Eraser, Pencil, Laser",
            ),
            ("V", "Switch to Select tool (Box marquee or Freeform Lasso)"),
            ("[ / ]", "Decrease / Increase active nib size"),
            (
                "Draw & Hold",
                "Pause at the end of a stroke to snap shapes or lines",
            ),
            (
                "Ctrl+C / X / V / D",
                "Copy, cut, paste, or duplicate selected items",
            ),
            ("Shift + Drag", "Snap current stroke to a straight line"),
            ("Space / Middle", "Pan canvas view"),
            ("Ctrl + Wheel", "Zoom canvas smoothly around cursor"),
            (
                "Ctrl + + / - / 0",
                "Scale app interface larger, smaller, or reset to 100%",
            ),
            ("Ctrl+Z / Ctrl+Y", "Undo / Redo stroke or page edits"),
            ("P", "Open or close Pages sidebar"),
            ("S", "Open or close this Settings drawer"),
            ("H", "Return to Notebooks Library home screen"),
            ("Esc", "Close active customization or export modal"),
        ];
        for (keys, desc) in shortcuts {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(102.0, 0.0),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        ui.label(
                            RichText::new(keys)
                                .size(10.5)
                                .monospace()
                                .strong()
                                .color(t.accent),
                        );
                    },
                );
                ui.add(egui::Label::new(RichText::new(desc).size(11.0).color(t.text_muted)).wrap());
            });
        }
    });
}

fn draw_settings_header(app: &mut DrawCraftApp, ui: &mut egui::Ui, kind: PenKind, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.set_height(28.0);
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Settings, t.accent);
        ui.label(RichText::new("Settings").size(17.0).strong().color(t.text));
        if app.view.settings_tab == SettingsTab::Pen {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, Icon::Reset, Some("Reset Tool"), false, true, t)
                    .on_hover_text(
                        "Restore factory default settings for the currently selected pen.",
                    )
                    .clicked()
                {
                    let mut defaults = PenSettings::for_kind(kind);
                    if let Some(size) = UserStudioConfig::DEFAULT_PEN_SIZES.get(kind.index()) {
                        defaults.size = *size;
                    }
                    defaults.color = app.session.pen(kind).color;
                    let _ = app.session.execute(
                        "pen.setSettings",
                        &json!({ "kind": kind, "settings": defaults }),
                    );
                }
            });
        }
    });
    ui.label(
        RichText::new("Input stabilization, studio appearance, and live telemetry.")
            .size(11.5)
            .color(t.text_muted),
    );
    ui.add_space(6.0);

    let tabs = [
        (
            SettingsTab::Pen,
            Icon::FountainPen,
            "Pen",
            "Input stabilization settings and live stroke preview",
        ),
        (
            SettingsTab::Appearance,
            Icon::Palette,
            "Appearance",
            "Studio theme, app interface scale, and automatic canvas color contrast",
        ),
        (
            SettingsTab::Telemetry,
            Icon::Telemetry,
            "Telemetry",
            "Live stylus telemetry and keyboard shortcuts",
        ),
    ];
    let active_idx = tabs
        .iter()
        .position(|(tab, _, _, _)| *tab == app.view.settings_tab);
    let options: Vec<SwitchOption<'_>> = tabs
        .iter()
        .map(|(_, icon, label, tip)| SwitchOption::new(Some(*icon), Some(label), tip, true))
        .collect();
    let (clicked, _) = segmented_switch(
        ui,
        "inspector_settings_tab_switch",
        &options,
        active_idx,
        SwitchLayout::EqualWidth,
        t,
    );
    if let Some(idx) = clicked
        && let Some((tab, _, _, _)) = tabs.get(idx)
    {
        app.view.settings_tab = *tab;
    }
}

pub(crate) fn export_target_with_ext(stem: &str, ext: &str) -> String {
    let trimmed = stem.trim();
    let base = trimmed
        .strip_suffix(".pdf")
        .or_else(|| trimmed.strip_suffix(".svg"))
        .or_else(|| trimmed.strip_suffix(".png"))
        .or_else(|| trimmed.strip_suffix(".dcraft"))
        .or_else(|| trimmed.strip_suffix(".scraft"))
        .or_else(|| trimmed.strip_suffix(".ncraft"))
        .unwrap_or(trimmed);
    let clean = if base.trim().is_empty() {
        "exports/Quick_Notes"
    } else {
        base.trim()
    };
    format!("{clean}.{ext}")
}

pub(crate) fn title_to_export_stem(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if (ch == ' ' || ch == '-' || ch == '_') && !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let clean = slug.trim_matches('_');
    if clean.is_empty() {
        "exports/Quick_Notes".to_string()
    } else {
        format!("exports/{clean}")
    }
}

pub(crate) fn title_to_default_filename(title: &str, ext: &str) -> String {
    let stem = title_to_export_stem(title);
    let base = stem.strip_prefix("exports/").unwrap_or(&stem);
    let clean_ext = ext.trim_start_matches('.');
    if clean_ext.is_empty() {
        base.to_string()
    } else {
        format!("{base}.{clean_ext}")
    }
}

pub(crate) const DEFAULT_INSPECTOR_WIDTH: f32 = 372.0;

pub fn show(app: &mut DrawCraftApp, ui: &mut egui::Ui) {
    let _ = show_inner(app, ui);
}

pub(crate) fn show_inner(app: &mut DrawCraftApp, ui: &mut egui::Ui) -> Rect {
    let t = Tokens::for_theme(app.session.prefs.theme);
    let prev_open = app.view.inspector_open;
    let mut panel_open = prev_open;
    let reveal_id = ui.make_persistent_id("drawcraft_inspector_reveal");
    let reveal = crate::motion::panel_reveal_factor(ui.ctx(), reveal_id, panel_open);
    let content_opacity = crate::motion::panel_content_opacity(reveal);

    let frame = Frame::new()
        .fill(t.panel_bg)
        .inner_margin(Margin::same(12))
        .stroke(Stroke::new(1.0, t.border));

    let rect = crate::fixed_side_panel(
        ui,
        if app.session.prefs.left_handed {
            egui::Panel::left("drawcraft_inspector")
        } else {
            egui::Panel::right("drawcraft_inspector")
        }
        .drag_to_open(false),
        DEFAULT_INSPECTOR_WIDTH,
        frame,
    )
    .show_collapsible(ui, &mut panel_open, |ui| {
        ui.multiply_opacity(content_opacity);
        let kind = app.session.active_pen_kind();
        draw_settings_header(app, ui, kind, &t);
        ui.separator();

        let dt = ui.ctx().input(|i| i.stable_dt);
        if app
            .view
            .motion
            .settings_tab
            .advance(app.view.settings_tab, dt)
        {
            ui.ctx().request_repaint();
        }
        let tab_opacity = app.view.motion.settings_tab.opacity();

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.scope(|ui| {
                ui.multiply_opacity(tab_opacity);
                match app.view.settings_tab {
                    SettingsTab::Pen => {
                        draw_live_stroke_preview(app, ui, kind, &t);
                        ui.add_space(8.0);
                        draw_pen_smoothing_card(app, ui, kind, &t);
                        if app.is_stylus_connected() {
                            ui.add_space(8.0);
                            draw_stylus_shortcuts_card(app, ui, &t);
                        }
                    }
                    SettingsTab::Appearance => {
                        draw_appearance_card(app, ui, &t);
                        ui.add_space(8.0);
                        crate::study_ui::draw_study_prefs_card(app, ui, &t);
                        ui.add_space(8.0);
                        draw_app_scale_card(app, ui, &t);
                    }
                    SettingsTab::Telemetry => {
                        draw_live_telemetry_card(app, ui, &t);
                        ui.add_space(8.0);
                        draw_shortcuts_card(ui, &t);
                    }
                }
            });
        });
    })
    .map_or(Rect::NOTHING, |resp| resp.response.rect);
    if panel_open != prev_open {
        app.view.inspector_open = panel_open;
    }
    rect
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod auto_contrast_ui_tests {
    use super::*;
    use crate::{AppScreen, SettingsTab, motion};
    use drawcraft_color::Color;
    use drawcraft_doc::{PaperColor, UiTheme};
    use egui::{Pos2, RawInput, Rect, Shape, Vec2};
    use serde_json::json;

    fn find_text_rect_in_shapes(
        shapes: &[egui::epaint::ClippedShape],
        needle: &str,
    ) -> Option<Rect> {
        fn visit(shape: &Shape, needle: &str) -> Option<Rect> {
            match shape {
                Shape::Text(t) if t.galley.text().contains(needle) => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()))
                }
                Shape::Vec(list) => list.iter().find_map(|s| visit(s, needle)),
                _ => None,
            }
        }
        shapes.iter().find_map(|cs| visit(&cs.shape, needle))
    }

    fn run_editor_frame(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        mut input: RawInput,
        time_s: &mut f64,
    ) -> Vec<egui::epaint::ClippedShape> {
        *time_s += 0.5;
        input.time = Some(*time_s);
        let mut out = ctx.run_ui(input, |ui| app.show_ui(ui));
        let shapes = std::mem::take(&mut out.shapes);
        out.drop_without_applying_deltas();
        shapes
    }

    fn base_input() -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
            ..Default::default()
        }
    }

    fn click_at(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        pos: Pos2,
        time_s: &mut f64,
    ) -> Vec<egui::epaint::ClippedShape> {
        let mut down = base_input();
        down.events.push(egui::Event::PointerMoved(pos));
        down.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = run_editor_frame(ctx, app, down, time_s);

        let mut up = base_input();
        up.events.push(egui::Event::PointerMoved(pos));
        up.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = run_editor_frame(ctx, app, up, time_s);
        run_editor_frame(ctx, app, base_input(), time_s)
    }

    fn sample_opaque_stroke_color(app: &DrawCraftApp, stroke_id: drawcraft_doc::StrokeId) -> Color {
        let (_, _, ribbon) = app
            .mesh_cache
            .get(&stroke_id)
            .expect("rendered stroke mesh should be cached");
        let v = ribbon
            .vertices
            .iter()
            .max_by(|a, b| a.rgba[3].total_cmp(&b.rgba[3]))
            .expect("mesh should have vertices");
        Color::new(v.rgba[0], v.rgba[1], v.rgba[2], v.rgba[3])
    }

    #[test]
    fn appearance_checkbox_toggles_auto_contrast_and_recolors_green_ink_in_dark_theme() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.view.screen = AppScreen::Editor;
        app.view.motion.screen = motion::ScreenTransition::new(AppScreen::Editor);
        app.view.inspector_open = true;
        app.view.settings_tab = SettingsTab::Appearance;

        // Draw a Forest Green stroke (#1D7A46) on WarmCream paper, then switch to Dark theme.
        let green_hex = "#1D7A46";
        let green = Color::from_hex(green_hex).unwrap();
        app.session
            .execute("tool.select", &json!({ "tool": "FountainPen" }))
            .unwrap();
        app.session
            .execute(
                "pen.setSettings",
                &json!({ "kind": "FountainPen", "color_hex": green_hex }),
            )
            .unwrap();
        let draw_res = app
            .session
            .execute(
                "stroke.draw",
                &json!({
                    "kind": "FountainPen",
                    "points": [
                        { "x": 120.0, "y": 140.0, "pressure": 1.0, "time_ms": 0.0 },
                        { "x": 200.0, "y": 150.0, "pressure": 1.0, "time_ms": 16.0 },
                        { "x": 280.0, "y": 160.0, "pressure": 1.0, "time_ms": 32.0 }
                    ]
                }),
            )
            .unwrap();
        let stroke_id = drawcraft_doc::StrokeId(draw_res["strokeId"].as_u64().unwrap());

        app.session
            .execute("theme.set", &json!({ "theme": "Dark" }))
            .unwrap();
        assert!(!app.session.prefs.auto_contrast_with_theme);

        // With checkbox default OFF, Dark theme leaves green ink canonical (#1D7A46).
        let mut time_s = 1.0_f64;
        let _ = run_editor_frame(&ctx, &mut app, base_input(), &mut time_s);
        let shapes = run_editor_frame(&ctx, &mut app, base_input(), &mut time_s);
        let check_rect = find_text_rect_in_shapes(&shapes, "Auto-adapt canvas colors to theme")
            .expect("Appearance checkbox label should be rendered on Appearance settings tab");
        let off_color = sample_opaque_stroke_color(&app, stroke_id);
        assert!(
            (off_color.r - green.r).abs() < 0.02
                && (off_color.g - green.g).abs() < 0.02
                && (off_color.b - green.b).abs() < 0.02,
            "when checkbox is OFF, ink must remain canonical green, got {:?}",
            off_color
        );

        // Click checkbox ON -> adapts paper to dark and lifts green ink for high contrast.
        let shapes_on = click_at(&ctx, &mut app, check_rect.center(), &mut time_s);
        assert!(app.session.prefs.auto_contrast_with_theme);
        assert_eq!(app.session.prefs.theme, UiTheme::Dark);

        let dark_bg = PaperColor::WarmCream
            .resolved_colors(UiTheme::Dark, true)
            .background;
        let on_color = sample_opaque_stroke_color(&app, stroke_id);
        assert!(
            on_color.g > on_color.r && on_color.g > on_color.b,
            "adapted ink must stay dominantly green, got {:?}",
            on_color
        );
        assert!(
            drawcraft_color::contrast_ratio(on_color, dark_bg) >= 6.49,
            "adapted green contrast ratio on dark paper too low: {:.2}",
            drawcraft_color::contrast_ratio(on_color, dark_bg)
        );

        // Click checkbox OFF again -> immediately invalidates cache and restores canonical ink.
        let check_rect_on =
            find_text_rect_in_shapes(&shapes_on, "Auto-adapt canvas colors to theme")
                .expect("Appearance checkbox label should remain visible after toggle");
        let _ = click_at(&ctx, &mut app, check_rect_on.center(), &mut time_s);
        assert!(!app.session.prefs.auto_contrast_with_theme);
        let restored = sample_opaque_stroke_color(&app, stroke_id);
        assert!(
            (restored.r - green.r).abs() < 0.02
                && (restored.g - green.g).abs() < 0.02
                && (restored.b - green.b).abs() < 0.02
        );
    }

    #[test]
    fn clicking_smoothing_preset_cards_updates_active_pen_and_preview() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.view.screen = AppScreen::Editor;
        app.view.motion.screen = motion::ScreenTransition::new(AppScreen::Editor);
        app.view.inspector_open = true;
        app.view.settings_tab = SettingsTab::Pen;

        let kind = app.session.active_pen_kind();
        assert_eq!(app.session.pen(kind).smoothing.buffer_size, 10);
        assert_eq!(
            app.session.pen(kind).smoothing.preset(),
            StabilizationPreset::ArithmeticBuffer10
        );

        let mut time_s = 1.0_f64;
        let _ = run_editor_frame(&ctx, &mut app, base_input(), &mut time_s);
        let mut shapes = run_editor_frame(&ctx, &mut app, base_input(), &mut time_s);

        for &preset in &[
            StabilizationPreset::ModeledSpring,
            StabilizationPreset::CurvedBezier,
            StabilizationPreset::SimpleDirect,
            StabilizationPreset::ArithmeticBuffer10,
        ] {
            let rect = find_text_rect_in_shapes(&shapes, preset.label())
                .expect("Preset card label should be rendered");
            shapes = click_at(&ctx, &mut app, rect.center(), &mut time_s);
            assert_eq!(app.session.pen(kind).smoothing.preset(), preset);
            assert_eq!(
                app.session.pen(kind).smoothing,
                preset.to_smoothing_config()
            );
        }
    }

    #[test]
    fn settings_three_way_tab_switch_cycles_between_pen_appearance_and_telemetry() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.view.screen = AppScreen::Editor;
        app.view.motion.screen = motion::ScreenTransition::new(AppScreen::Editor);
        app.view.inspector_open = true;
        app.view.settings_tab = SettingsTab::Pen;

        let mut time_s = 1.0_f64;
        let _ = run_editor_frame(&ctx, &mut app, base_input(), &mut time_s);
        let shapes_pen = run_editor_frame(&ctx, &mut app, base_input(), &mut time_s);

        let appearance_tab = find_text_rect_in_shapes(&shapes_pen, "Appearance")
            .expect("Appearance tab should be rendered in 3-way switch");
        let shapes_app = click_at(&ctx, &mut app, appearance_tab.center(), &mut time_s);
        assert_eq!(app.view.settings_tab, SettingsTab::Appearance);
        assert!(
            find_text_rect_in_shapes(&shapes_app, "Auto-adapt canvas colors to theme").is_some()
        );

        let telemetry_tab = find_text_rect_in_shapes(&shapes_app, "Telemetry")
            .expect("Telemetry tab should be rendered in 3-way switch");
        let shapes_tel = click_at(&ctx, &mut app, telemetry_tab.center(), &mut time_s);
        assert_eq!(app.view.settings_tab, SettingsTab::Telemetry);
        assert!(find_text_rect_in_shapes(&shapes_tel, "Live Stylus Telemetry").is_some());

        let pen_tab = find_text_rect_in_shapes(&shapes_tel, "Pen")
            .expect("Pen tab should be rendered in 3-way switch");
        let shapes_back = click_at(&ctx, &mut app, pen_tab.center(), &mut time_s);
        assert_eq!(app.view.settings_tab, SettingsTab::Pen);
        assert!(find_text_rect_in_shapes(&shapes_back, "Input stabilization").is_some());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn collect_text_shapes(shapes: &[egui::epaint::ClippedShape]) -> String {
        let mut out = String::new();
        fn visit(shape: &egui::Shape, buf: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    buf.push_str(text.galley.text());
                    buf.push('\n');
                }
                egui::Shape::Vec(items) => {
                    for s in items {
                        visit(s, buf);
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

    fn render_inspector_tab(app: &mut DrawCraftApp, tab: SettingsTab) -> (Rect, String) {
        app.view.inspector_open = true;
        app.view.settings_tab = tab;
        let ctx = egui::Context::default();
        let mut last_texts = String::new();
        let mut panel_rect = Rect::NOTHING;
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
                panel_rect = show_inner(app, ui);
            });
            let has_shapes = !out.shapes.is_empty();
            last_texts = collect_text_shapes(&out.shapes);
            out.drop_without_applying_deltas();
            assert!(has_shapes);
            assert!(
                (panel_rect.width() - DEFAULT_INSPECTOR_WIDTH).abs() <= 0.5
                    && panel_rect.right() <= 1280.5,
                "tab {tab:?} inspector rect {panel_rect:?} overflowed or differed from default {DEFAULT_INSPECTOR_WIDTH}",
            );
        }
        assert!(last_texts.lines().any(|line| line == "Pen"));
        assert!(last_texts.lines().any(|line| line == "Appearance"));
        assert!(!last_texts.lines().any(|line| line == "Page"));
        assert!(last_texts.lines().any(|line| line == "Telemetry"));
        (panel_rect, last_texts)
    }

    #[test]
    fn input_stabilization_renders_four_curated_presets_without_manual_controls() {
        let mut app = DrawCraftApp::default();
        for kind in PenKind::ALL {
            assert_eq!(
                app.session.pen(*kind).smoothing,
                drawcraft_doc::SmoothingConfig::default()
            );
            assert_eq!(app.session.pen(*kind).smoothing.buffer_size, 10);
        }

        let (_, pen_texts) = render_inspector_tab(&mut app, SettingsTab::Pen);
        assert!(pen_texts.contains("Input stabilization"));
        assert!(pen_texts.contains("Choose a curated stroke smoothing preset for the active pen."));
        for &preset in StabilizationPreset::ALL {
            assert!(
                pen_texts.contains(preset.label()),
                "missing preset label {}",
                preset.label()
            );
            assert!(
                pen_texts.contains(preset.badge()),
                "missing preset badge {}",
                preset.badge()
            );
            assert!(
                pen_texts.contains(preset.description()),
                "missing preset description {}",
                preset.description()
            );
        }

        // Manual tuning parameters must not be exposed in the UI.
        assert!(!pen_texts.contains("Averaging method"));
        assert!(!pen_texts.contains("Buffer size"));
        assert!(!pen_texts.contains("Preprocessor"));
        assert!(!pen_texts.contains("Finalize the stroke"));
        assert!(!pen_texts.contains("Deadzone radius"));
        assert!(!pen_texts.lines().any(|line| line == "Drag"));
        assert!(!pen_texts.lines().any(|line| line == "Mass"));
    }

    #[test]
    fn telemetry_tab_renders_live_metrics_and_shortcuts() {
        let mut app = DrawCraftApp::default();
        app.stylus.telemetry.hardware_active = true;
        app.stylus.telemetry.pressure = f32::NAN;
        app.stylus.telemetry.tilt_x = f32::INFINITY;
        app.stylus.telemetry.tilt_y = -200.0;
        app.stylus.telemetry.rotation = f32::NEG_INFINITY;
        app.stylus.telemetry.velocity_px_ms = f32::NAN;
        app.stylus.telemetry.eraser_inverted = true;
        app.stylus.telemetry.barrel_held = true;

        let (_, tel_texts) = render_inspector_tab(&mut app, SettingsTab::Telemetry);
        assert!(tel_texts.contains("Hardware Stylus Active"));
        assert!(tel_texts.contains("Live Stylus Telemetry"));
        assert!(tel_texts.contains("Pressure"));
        assert!(tel_texts.contains("0% (0.00)"));
        assert!(tel_texts.contains("Tilt X"));
        assert!(tel_texts.contains("Tilt Y"));
        assert!(tel_texts.contains("-90.0°"));
        assert!(tel_texts.contains("Rotation"));
        assert!(tel_texts.contains("Speed"));
        assert!(tel_texts.contains("ACTIVE"));
        assert!(tel_texts.contains("HELD"));
        assert!(tel_texts.contains("Keyboard & Canvas Shortcuts"));
        assert!(!tel_texts.contains("Auto-adapt canvas colors to theme"));
        assert!(!tel_texts.contains("Paper Ruling"));
        assert!(!tel_texts.contains("NaN"));
        assert!(!tel_texts.contains("inf"));
    }

    #[test]
    fn appearance_tab_renders_theme_switch_and_auto_contrast_card() {
        let mut app = DrawCraftApp::default();
        let (_, app_texts) = render_inspector_tab(&mut app, SettingsTab::Appearance);
        assert!(app_texts.contains("Appearance"));
        assert!(app_texts.contains("Light"));
        assert!(app_texts.contains("Dark"));
        assert!(app_texts.contains("Auto-adapt canvas colors to theme"));
        assert!(app_texts.contains("App Scale"));
        assert!(app_texts.contains("100%"));
        assert!(app_texts.contains("80%"));
        assert!(app_texts.contains("125%"));
        assert!(app_texts.contains("150%"));
        assert!(app_texts.contains("175%"));
        assert!(!app_texts.contains("Live Stylus Telemetry"));
        assert!(!app_texts.contains("Keyboard & Canvas Shortcuts"));
    }

    fn find_text_center(shapes: &[egui::epaint::ClippedShape], needle: &str) -> Option<egui::Pos2> {
        fn visit(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == needle => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                egui::Shape::Vec(items) => items.iter().find_map(|s| visit(s, needle)),
                _ => None,
            }
        }
        shapes.iter().find_map(|c| visit(&c.shape, needle))
    }

    #[test]
    fn settings_header_has_pen_appearance_and_telemetry_tabs_on_single_row() {
        let mut app = DrawCraftApp::default();
        app.view.inspector_open = true;
        let ctx = egui::Context::default();
        let mut shapes = Vec::new();
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
                let _ = show_inner(&mut app, ui);
            });
            shapes = out.shapes.clone();
            out.drop_without_applying_deltas();
        }
        let pen_pos = find_text_center(&shapes, "Pen").unwrap();
        let app_pos = find_text_center(&shapes, "Appearance").unwrap();
        let tel_pos = find_text_center(&shapes, "Telemetry").unwrap();
        assert!((pen_pos.y - app_pos.y).abs() <= 1.0);
        assert!((pen_pos.y - tel_pos.y).abs() <= 1.0);
        assert!(pen_pos.x < app_pos.x && app_pos.x < tel_pos.x);
        assert!(find_text_center(&shapes, "Page").is_none());
    }

    #[test]
    fn pen_tab_stabilization_settings_only_update_active_pen() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);

        app.session
            .execute("tool.select", &json!({ "tool": "BallPen" }))
            .unwrap();
        let custom_color = drawcraft_color::Color::from_hex("#1D4ED8").unwrap();
        app.session
            .execute(
                "pen.setSettings",
                &json!({ "kind": PenKind::BallPen, "size": 8.0, "color": custom_color }),
            )
            .unwrap();

        let before = app.session.pen(PenKind::BallPen).clone();
        app.session
            .execute(
                "pen.setSmoothing",
                &json!({
                    "kind": app.session.active_pen_kind(),
                    "preset": StabilizationPreset::ModeledSpring,
                }),
            )
            .unwrap();
        let after = app.session.pen(PenKind::BallPen);

        assert_eq!(after.smoothing.preset(), StabilizationPreset::ModeledSpring);
        assert_eq!(
            after.smoothing,
            StabilizationPreset::ModeledSpring.to_smoothing_config()
        );
        assert!((after.size - before.size).abs() < 1e-4);
        assert_eq!(after.color, before.color);
        assert_eq!(after.nib_flatness, before.nib_flatness);
        assert_eq!(after.pressure_enabled, before.pressure_enabled);
        assert_eq!(after.pressure_curve, before.pressure_curve);
        assert_eq!(
            app.session.pen(PenKind::FountainPen).smoothing,
            drawcraft_doc::SmoothingConfig::default()
        );
    }

    #[test]
    fn app_scale_presets_and_reset_update_session_scale() {
        let mut app = DrawCraftApp::default();
        app.view.inspector_open = true;
        app.view.settings_tab = SettingsTab::Appearance;
        let ctx = egui::Context::default();

        let render_step =
            |ctx: &egui::Context, app: &mut DrawCraftApp, t: f64, events: Vec<egui::Event>| {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    time: Some(t),
                    events,
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    let _ = show_inner(app, ui);
                });
                let shapes = out.shapes.clone();
                out.drop_without_applying_deltas();
                shapes
            };

        let mut shapes = Vec::new();
        for f in 0..4 {
            shapes = render_step(&ctx, &mut app, f as f64 * 0.03, Vec::new());
        }
        let preset_125 = find_text_center(&shapes, "125%").expect("125% preset button");

        for (pressed, t) in [(true, 0.20), (false, 0.24)] {
            let _ = render_step(
                &ctx,
                &mut app,
                t,
                vec![
                    egui::Event::PointerMoved(preset_125),
                    egui::Event::PointerButton {
                        pos: preset_125,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!((app.session.prefs.ui_scale - 1.25).abs() < 1e-4);
        shapes = render_step(&ctx, &mut app, 0.28, Vec::new());
        let reset_pos = find_text_center(&shapes, "Reset").expect("Reset button");

        for (pressed, t) in [(true, 0.35), (false, 0.39)] {
            let _ = render_step(
                &ctx,
                &mut app,
                t,
                vec![
                    egui::Event::PointerMoved(reset_pos),
                    egui::Event::PointerButton {
                        pos: reset_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!((app.session.prefs.ui_scale - 1.0).abs() < 1e-4);
    }

    #[test]
    fn stylus_shortcuts_card_visibility_hot_unplug_dropdown_and_reset() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let ctx = egui::Context::default();
        let connected = Arc::new(AtomicBool::new(false));
        let connected_hook = Arc::clone(&connected);

        let hooks = crate::PlatformHooks {
            connected_pen_devices: Some(Box::new(move || {
                if connected_hook.load(Ordering::Relaxed) {
                    vec![crate::stylus::ConnectedStylusInfo {
                        name: "Active Pen Digitizer".to_string(),
                        is_external: true,
                    }]
                } else {
                    Vec::new()
                }
            })),
            ..crate::PlatformHooks::default()
        };
        let mut app = DrawCraftApp::new(drawcraft_engine::Session::default(), hooks);
        app.view.screen = crate::AppScreen::Editor;
        app.view.inspector_open = true;
        app.view.settings_tab = SettingsTab::Pen;

        let render_step =
            |ctx: &egui::Context, app: &mut DrawCraftApp, t: f64, events: Vec<egui::Event>| {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 1100.0),
                    )),
                    time: Some(t),
                    events,
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    if app.last_ctx.is_none() {
                        app.last_ctx = Some(ui.ctx().clone());
                    }
                    app.poll_connected_stylus_devices(t);
                    let _ = show_inner(app, ui);
                });
                let texts = collect_text_shapes(&out.shapes);
                let shapes = out.shapes.clone();
                out.drop_without_applying_deltas();
                (texts, shapes)
            };

        // 1. Disconnected: Pen tab shows Input stabilization only and hides Stylus shortcuts.
        let (texts_off, _) = render_step(&ctx, &mut app, 0.02, Vec::new());
        assert!(texts_off.contains("Input stabilization"));
        assert!(!texts_off.contains("Stylus shortcuts"));

        // 2. Connect stylus and re-poll after interval: card appears with device name and 3 rows.
        connected.store(true, Ordering::Relaxed);
        app.stylus.telemetry.barrel_held = true;
        let (texts_on, _) = render_step(&ctx, &mut app, 2.0, Vec::new());
        assert!(texts_on.contains("Stylus shortcuts"));
        assert!(texts_on.contains("Connected · Active Pen Digitizer"));
        assert!(texts_on.contains("Side Button 1"));
        assert!(texts_on.contains("Side Button 2"));
        assert!(texts_on.contains("Eraser End"));
        assert!(texts_on.contains("Pressed"));

        // 3. Open Side Button 1 dropdown ("Toggle Pen / Eraser", the default) and pick "Undo".
        app.stylus.telemetry.barrel_held = false;
        let (_, shapes_idle) = render_step(&ctx, &mut app, 2.05, Vec::new());
        let combo_pos =
            find_text_center(&shapes_idle, "Toggle Pen / Eraser").expect("Side Button 1 combo box");

        for (pressed, t) in [(true, 2.10), (false, 2.14)] {
            let _ = render_step(
                &ctx,
                &mut app,
                t,
                vec![
                    egui::Event::PointerMoved(combo_pos),
                    egui::Event::PointerButton {
                        pos: combo_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        let (_, shapes_popup) = render_step(&ctx, &mut app, 2.18, Vec::new());
        let undo_pos = find_text_center(&shapes_popup, "Undo").expect("Undo dropdown item");
        for (pressed, t) in [(true, 2.22), (false, 2.26)] {
            let _ = render_step(
                &ctx,
                &mut app,
                t,
                vec![
                    egui::Event::PointerMoved(undo_pos),
                    egui::Event::PointerButton {
                        pos: undo_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(
            app.session.prefs.stylus_buttons.barrel_button,
            StylusButtonAction::Undo
        );

        // 4. Click Reset button inside Stylus shortcuts card -> restores default config.
        let (_, shapes_custom) = render_step(&ctx, &mut app, 2.30, Vec::new());
        let reset_pos = find_text_center(&shapes_custom, "Reset").expect("Reset shortcuts button");
        for (pressed, t) in [(true, 2.34), (false, 2.38)] {
            let _ = render_step(
                &ctx,
                &mut app,
                t,
                vec![
                    egui::Event::PointerMoved(reset_pos),
                    egui::Event::PointerButton {
                        pos: reset_pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(
            app.session.prefs.stylus_buttons,
            StylusButtonConfig::default()
        );

        // 5. Hot-unplugging hides Stylus shortcuts again.
        connected.store(false, Ordering::Relaxed);
        let (texts_unplugged, _) = render_step(&ctx, &mut app, 4.5, Vec::new());
        assert!(!texts_unplugged.contains("Stylus shortcuts"));
    }
}
