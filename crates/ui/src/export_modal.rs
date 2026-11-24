//! Clean, objective export format selector rendered inside the standard modal shell.

use egui::{CornerRadius, Margin, RichText, Sense, Stroke, StrokeKind, Vec2, pos2};
use serde_json::json;

use crate::icons::{Icon, paint_icon};
use crate::inspector::{export_target_with_ext, title_to_default_filename};
use crate::modal::{ActiveModal, primary_action_button, secondary_action_button};
use crate::motion::{hover_factor, lerp_color};
use crate::theme::Tokens;
use crate::{DrawCraftApp, FileDialogKind};

#[derive(Clone, Copy)]
struct ExportFormatSpec {
    kind: FileDialogKind,
    label: &'static str,
    extension_tag: &'static str,
    tooltip: &'static str,
}

const EXPORT_FORMATS: [ExportFormatSpec; 5] = [
    ExportFormatSpec {
        kind: FileDialogKind::Pdf,
        label: "PDF document",
        extension_tag: ".pdf",
        tooltip: "Export all pages with ink, text, shapes, and images as a multi-page PDF.",
    },
    ExportFormatSpec {
        kind: FileDialogKind::Png,
        label: "PNG image",
        extension_tag: ".png",
        tooltip: "Export the active page as a high-resolution PNG image.",
    },
    ExportFormatSpec {
        kind: FileDialogKind::Svg,
        label: "SVG vector",
        extension_tag: ".svg",
        tooltip: "Export the active page as scalable vector artwork.",
    },
    ExportFormatSpec {
        kind: FileDialogKind::Dcraft,
        label: "Notebook file",
        extension_tag: ".dcraft",
        tooltip: "Save a portable DrawCraft notebook archive.",
    },
    ExportFormatSpec {
        kind: FileDialogKind::Package,
        label: "Complete package",
        extension_tag: ".dcraftpkg",
        tooltip: "Export all active notebooks, trashed items, folders, templates, and studio preferences into one backup file.",
    },
];

/// Opens the standard Export modal pre-scoped to the ordered 0-based `pages` selection.
pub(crate) fn open_for_pages(app: &mut DrawCraftApp, pages: Vec<usize>) {
    let page_count = app.session.doc.page_count().max(1);
    let clean = sanitize_export_pages(&pages, page_count);
    app.view.export_target_pages = if clean.is_empty() { None } else { Some(clean) };
    crate::modal::open(app, ActiveModal::Export);
}

fn sanitize_export_pages(raw: &[usize], page_count: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(raw.len().min(page_count));
    for &idx in raw {
        if idx < page_count && !out.contains(&idx) {
            out.push(idx);
        }
    }
    out
}

/// Renders the Export Notebook modal body. Returns `true` if the modal should close.
pub(crate) fn render_body(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    if app.view.active_modal == Some(ActiveModal::Export) {
        let (down, up) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
            )
        });
        if down || up {
            let cur_idx = EXPORT_FORMATS
                .iter()
                .position(|spec| spec.kind == app.view.export_format)
                .unwrap_or(0);
            let next_idx = if down {
                (cur_idx + 1).min(EXPORT_FORMATS.len() - 1)
            } else {
                cur_idx.saturating_sub(1)
            };
            if let Some(next_spec) = EXPORT_FORMATS.get(next_idx)
                && next_spec.kind != app.view.export_format
            {
                app.view.export_format = next_spec.kind;
                app.view.export_feedback = None;
            }
        }
    }

    let page_count = app.session.doc.page_count().max(1);
    let active_num = app.session.doc.active_page_index() + 1;
    let target_pages = app
        .view
        .export_target_pages
        .as_deref()
        .map(|raw| sanitize_export_pages(raw, page_count))
        .filter(|v| !v.is_empty());

    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(10.0)
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            for spec in EXPORT_FORMATS {
                let scope = format_scope_label(
                    spec.kind,
                    page_count,
                    active_num,
                    target_pages.as_deref(),
                    app.view.export_separate_files,
                );
                let selected = app.view.export_format == spec.kind;
                if draw_format_radio_row(ui, spec, &scope, selected, t).clicked() {
                    app.view.export_format = spec.kind;
                    app.view.export_feedback = None;
                }
            }
        });

    ui.add_space(8.0);
    if app.view.export_format == FileDialogKind::Package {
        egui::Frame::NONE
            .fill(t.panel_bg)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(10.0)
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Complete Backup Package: exports all active notebooks, Trash items, folders, custom templates, and studio settings together.",
                        )
                        .size(11.0)
                        .color(t.text_muted),
                    )
                    .wrap(),
                );
            });
    } else {
        draw_page_scope_options(app, ui, page_count, target_pages.as_deref(), t);
    }

    if let Some((is_error, message)) = app.view.export_feedback.as_ref() {
        ui.add_space(8.0);
        draw_feedback_banner(ui, *is_error, message, t);
    }

    false
}

/// Renders the Export Notebook modal footer (`Cancel` and `Export`). Returns `true` when closed.
pub(crate) fn render_footer(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let enter_pressed = app.view.active_modal == Some(ActiveModal::Export)
        && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut should_close = false;
    let mut export_requested = enter_pressed;

    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if primary_action_button(ui, "Export", true, t).clicked() {
                export_requested = true;
            }
            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                should_close = true;
            }
        });
    });

    if !should_close && export_requested {
        if trigger_export_or_save(app, app.view.export_format) {
            should_close = true;
        } else {
            ui.ctx().request_repaint();
        }
    }

    should_close
}

/// Exports a selected subset of 0-based `page_indices` to a multi-page PDF file via native/web hooks.
#[cfg(test)]
pub(crate) fn trigger_export_selected_pages_pdf(app: &mut DrawCraftApp, page_indices: &[usize]) {
    if page_indices.is_empty() {
        return;
    }
    let stem = title_to_default_filename(&app.session.doc.title, "");
    let clean_stem = stem.trim_end_matches('.');
    let default_name = format!("{clean_stem}_selected.pdf");
    if let Some(pick_save) = app.hooks.pick_save_path.as_mut() {
        let Some(path) = pick_save(FileDialogKind::Pdf, &default_name) else {
            app.session.last_export_status = Some("Export cancelled.".to_string());
            return;
        };
        if let Err(err) = app.session.execute(
            "export.pdf",
            &json!({ "path": path.to_string_lossy(), "pages": page_indices }),
        ) {
            app.session.last_export_status = Some(err.to_string());
        }
        return;
    }
    if let Some(save_bytes) = app.hooks.save_bytes_dialog.as_mut() {
        match app.session.export_pdf_bytes_for_pages(Some(page_indices)) {
            Ok(bytes) => save_bytes(FileDialogKind::Pdf, &default_name, bytes),
            Err(err) => app.session.last_export_status = Some(err.to_string()),
        }
        return;
    }
    let base_stem = export_target_with_ext(&app.view.export_path_stem, "");
    let path = format!("{}_selected.pdf", base_stem.trim_end_matches('.'));
    if let Err(err) = app.session.execute(
        "export.pdf",
        &json!({ "path": path, "pages": page_indices }),
    ) {
        app.session.last_export_status = Some(err.to_string());
    }
}

fn numbered_export_filename(stem: &str, order_idx: usize, page_idx: usize, ext: &str) -> String {
    let clean_stem = stem.trim_end_matches('.');
    let clean_ext = ext.trim_start_matches('.');
    format!(
        "{clean_stem}_{:02}_page{}.{}",
        order_idx + 1,
        page_idx + 1,
        clean_ext
    )
}

fn numbered_export_path(
    base_path: &std::path::Path,
    order_idx: usize,
    page_idx: usize,
    ext: &str,
) -> std::path::PathBuf {
    let stem = base_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("notebook");
    let file_name = numbered_export_filename(stem, order_idx, page_idx, ext);
    match base_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(file_name),
        _ => std::path::PathBuf::from(file_name),
    }
}

fn command_and_params_for_pages(
    kind: FileDialogKind,
    path: &str,
    pages: Option<&[usize]>,
) -> (&'static str, serde_json::Value) {
    let cmd = command_for_kind(kind);
    let params = match (kind, pages) {
        (FileDialogKind::Pdf | FileDialogKind::Dcraft, Some(list)) if !list.is_empty() => {
            json!({ "path": path, "pages": list })
        }
        (FileDialogKind::Png | FileDialogKind::Svg, Some([page_idx, ..])) => {
            json!({ "path": path, "page": page_idx })
        }
        _ => json!({ "path": path }),
    };
    (cmd, params)
}

fn export_bytes_for_pages(
    session: &mut drawcraft_engine::Session,
    kind: FileDialogKind,
    pages: Option<&[usize]>,
) -> Result<Vec<u8>, drawcraft_engine::EngineError> {
    match kind {
        FileDialogKind::Pdf => match pages {
            Some(list) if !list.is_empty() => session.export_pdf_bytes_for_pages(Some(list)),
            _ => session.export_pdf_bytes(),
        },
        FileDialogKind::Svg => {
            session.export_svg_bytes(pages.and_then(|list| list.first().copied()))
        }
        FileDialogKind::Png => {
            session.export_png_bytes(pages.and_then(|list| list.first().copied()))
        }
        FileDialogKind::Dcraft | FileDialogKind::Import => match pages {
            Some(list) if !list.is_empty() => session.save_notebook_bytes_for_pages(Some(list)),
            _ => session.save_notebook_bytes(),
        },
        FileDialogKind::Package => session.export_package_bytes(),
    }
}

fn set_export_error(app: &mut DrawCraftApp, err: impl std::fmt::Display) -> bool {
    let msg = err.to_string();
    app.session.last_export_status = Some(msg.clone());
    app.view.export_feedback = Some((true, msg));
    false
}

fn export_pages_separately(app: &mut DrawCraftApp, kind: FileDialogKind, pages: &[usize]) -> bool {
    let title_stem = title_to_default_filename(&app.session.doc.title, "");
    let default_name = title_to_default_filename(&app.session.doc.title, kind.extension());

    if let Some(pick_choice) = app
        .hooks
        .pick_save_path
        .as_mut()
        .map(|pick_save| pick_save(kind, &default_name))
    {
        let Some(base_path) = pick_choice else {
            let msg = "Export cancelled.".to_string();
            app.session.last_export_status = Some(msg.clone());
            app.view.export_feedback = Some((false, msg));
            return false;
        };
        for (order_idx, &page_idx) in pages.iter().enumerate() {
            let path = numbered_export_path(&base_path, order_idx, page_idx, kind.extension());
            let (cmd, params) =
                command_and_params_for_pages(kind, &path.to_string_lossy(), Some(&[page_idx]));
            if let Err(err) = app.session.execute(cmd, &params) {
                return set_export_error(app, err);
            }
        }
        app.session.last_export_status = Some(format!(
            "Exported {} separate {} files in selection order.",
            pages.len(),
            kind.extension()
        ));
        app.view.export_feedback = None;
        return true;
    }

    if app.hooks.save_bytes_dialog.is_some() {
        let mut prepared = Vec::with_capacity(pages.len());
        for (order_idx, &page_idx) in pages.iter().enumerate() {
            let file_name =
                numbered_export_filename(&title_stem, order_idx, page_idx, kind.extension());
            match export_bytes_for_pages(&mut app.session, kind, Some(&[page_idx])) {
                Ok(bytes) => prepared.push((file_name, bytes)),
                Err(err) => return set_export_error(app, err),
            }
        }
        if let Some(save_bytes) = app.hooks.save_bytes_dialog.as_mut() {
            for (file_name, bytes) in prepared {
                save_bytes(kind, &file_name, bytes);
            }
        }
        app.session.last_export_status = Some(format!(
            "Prepared {} separate {} files in selection order.",
            pages.len(),
            kind.extension()
        ));
        app.view.export_feedback = None;
        return true;
    }

    let base_stem = export_target_with_ext(&app.view.export_path_stem, "");
    for (order_idx, &page_idx) in pages.iter().enumerate() {
        let path = numbered_export_filename(&base_stem, order_idx, page_idx, kind.extension());
        let (cmd, params) = command_and_params_for_pages(kind, &path, Some(&[page_idx]));
        if let Err(err) = app.session.execute(cmd, &params) {
            return set_export_error(app, err);
        }
    }
    app.session.last_export_status = Some(format!(
        "Exported {} separate {} files in selection order.",
        pages.len(),
        kind.extension()
    ));
    app.view.export_feedback = None;
    true
}

fn trigger_export_or_save(app: &mut DrawCraftApp, kind: FileDialogKind) -> bool {
    let page_count = app.session.doc.page_count().max(1);
    let target_pages = app
        .view
        .export_target_pages
        .as_deref()
        .map(|raw| sanitize_export_pages(raw, page_count))
        .filter(|v| !v.is_empty());

    let separate_pages: Option<Vec<usize>> = if app.view.export_separate_files {
        Some(
            target_pages
                .clone()
                .unwrap_or_else(|| (0..page_count).collect()),
        )
    } else if matches!(kind, FileDialogKind::Png | FileDialogKind::Svg)
        && target_pages.as_ref().is_some_and(|p| p.len() > 1)
    {
        target_pages.clone()
    } else {
        None
    };

    if kind != FileDialogKind::Package
        && let Some(pages) = separate_pages
        && pages.len() > 1
    {
        return export_pages_separately(app, kind, &pages);
    }

    let default_name = if kind == FileDialogKind::Package {
        "DrawCraft_Complete_Package.dcraftpkg".to_string()
    } else {
        title_to_default_filename(&app.session.doc.title, kind.extension())
    };
    if let Some(pick_choice) = app
        .hooks
        .pick_save_path
        .as_mut()
        .map(|pick_save| pick_save(kind, &default_name))
    {
        let Some(path) = pick_choice else {
            let msg = "Export cancelled.".to_string();
            app.session.last_export_status = Some(msg.clone());
            app.view.export_feedback = Some((false, msg));
            return false;
        };
        let (cmd, params) =
            command_and_params_for_pages(kind, &path.to_string_lossy(), target_pages.as_deref());
        if let Err(err) = app.session.execute(cmd, &params) {
            return set_export_error(app, err);
        }
        app.view.export_feedback = None;
        return true;
    }

    if app.hooks.save_bytes_dialog.is_some() {
        let bytes_res = export_bytes_for_pages(&mut app.session, kind, target_pages.as_deref());
        return match bytes_res {
            Ok(bytes) => {
                if let Some(save_bytes) = app.hooks.save_bytes_dialog.as_mut() {
                    save_bytes(kind, &default_name, bytes);
                }
                app.view.export_feedback = None;
                true
            }
            Err(err) => set_export_error(app, err),
        };
    }

    let path = export_target_with_ext(&app.view.export_path_stem, kind.extension());
    let (cmd, params) = command_and_params_for_pages(kind, &path, target_pages.as_deref());
    if let Err(err) = app.session.execute(cmd, &params) {
        return set_export_error(app, err);
    }
    app.view.export_feedback = None;
    true
}

const fn command_for_kind(kind: FileDialogKind) -> &'static str {
    match kind {
        FileDialogKind::Pdf => "export.pdf",
        FileDialogKind::Svg => "export.svg",
        FileDialogKind::Png => "export.png",
        FileDialogKind::Dcraft | FileDialogKind::Import => "notebook.save",
        FileDialogKind::Package => "package.export",
    }
}

fn format_scope_label(
    kind: FileDialogKind,
    page_count: usize,
    active_num: usize,
    target_pages: Option<&[usize]>,
    separate_files: bool,
) -> String {
    if kind == FileDialogKind::Package {
        return "All saves & notebooks".to_string();
    }
    if let Some(pages) = target_pages {
        if pages.len() == 1 {
            return format!("Page {}", pages[0] + 1);
        }
        if separate_files || matches!(kind, FileDialogKind::Png | FileDialogKind::Svg) {
            return format!("{} separate files", pages.len());
        }
        return format!("{} selected pages", pages.len());
    }
    if separate_files && page_count > 1 {
        return format!("{page_count} separate files");
    }
    match kind {
        FileDialogKind::Pdf => {
            if page_count == 1 {
                "1 page".to_string()
            } else {
                format!("All {page_count} pages")
            }
        }
        FileDialogKind::Png | FileDialogKind::Svg => format!("Page {active_num}"),
        FileDialogKind::Dcraft | FileDialogKind::Import => "Full notebook".to_string(),
        FileDialogKind::Package => "All saves & notebooks".to_string(),
    }
}

fn draw_page_scope_options(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    page_count: usize,
    target_pages: Option<&[usize]>,
    t: &Tokens,
) {
    let split_count = target_pages.map_or(page_count, <[usize]>::len);
    let can_split = split_count > 1;
    let force_separate_image = matches!(
        app.view.export_format,
        FileDialogKind::Png | FileDialogKind::Svg
    ) && can_split
        && target_pages.is_some();

    egui::Frame::NONE
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;

            if let Some(pages) = target_pages {
                let summary = if pages.len() == 1 {
                    format!("Selected page: Page {}", pages[0] + 1)
                } else {
                    let order = pages
                        .iter()
                        .map(|idx| format!("Page {}", idx + 1))
                        .collect::<Vec<_>>()
                        .join(" → ");
                    format!("Selection order ({}): {order}", pages.len())
                };
                ui.add(
                    egui::Label::new(RichText::new(summary).size(11.5).strong().color(t.accent))
                        .wrap(),
                );
            }

            let mut checked = if force_separate_image {
                true
            } else {
                app.view.export_separate_files
            };
            let check_resp = ui
                .add_enabled(
                    can_split && !force_separate_image,
                    egui::Checkbox::new(
                        &mut checked,
                        RichText::new("Export pages as separate files")
                            .size(12.5)
                            .color(if can_split { t.text } else { t.text_muted }),
                    ),
                )
                .on_hover_text(
                    "Export each page as its own numbered file or combine them together in selection order",
                );
            if can_split && !force_separate_image && check_resp.changed() {
                app.view.export_separate_files = checked;
            }

            let hint = if !can_split {
                "Exports the selected single page."
            } else if checked {
                "Generate one numbered file per page preserving selection order."
            } else {
                "Combine exported pages into a single file preserving selection order."
            };
            ui.add(egui::Label::new(RichText::new(hint).size(11.0).color(t.text_muted)).wrap());
        });
}

fn draw_format_radio_row(
    ui: &mut egui::Ui,
    spec: ExportFormatSpec,
    scope: &str,
    selected: bool,
    t: &Tokens,
) -> egui::Response {
    let width = ui.available_width().max(160.0);
    let row_h = 44.0_f32;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, row_h), Sense::click());
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let hover_t = hover_factor(ctx, resp.id.with("hov"), resp.hovered() && !selected);
        let sel_t = hover_factor(ctx, resp.id.with("sel"), selected);
        let idle_fill = lerp_color(t.panel_bg, t.toolbar_bg, hover_t);
        let fill = lerp_color(idle_fill, t.accent_soft, sel_t);
        let stroke = if sel_t > 0.01 {
            Stroke::new(1.0, t.accent.gamma_multiply(0.55 * sel_t))
        } else {
            Stroke::NONE
        };
        let p = ui.painter();
        p.rect(
            rect,
            CornerRadius::same(7),
            fill,
            stroke,
            StrokeKind::Inside,
        );

        let radio_center = pos2(rect.left() + 20.0, rect.center().y);
        let outer_r = 7.5_f32;
        if selected {
            p.circle(
                radio_center,
                outer_r,
                t.field_bg,
                Stroke::new(1.8, t.accent),
            );
            p.circle_filled(radio_center, 3.8, t.accent);
        } else {
            let ring_c = lerp_color(t.text_muted.gamma_multiply(0.55), t.text_muted, hover_t);
            p.circle(radio_center, outer_r, t.field_bg, Stroke::new(1.4, ring_c));
        }

        let label_x = radio_center.x + outer_r + 12.0;
        let label_galley = p.layout_no_wrap(
            spec.label.to_string(),
            egui::FontId::proportional(13.0),
            t.text,
        );
        let ext_galley = p.layout_no_wrap(
            spec.extension_tag.to_string(),
            egui::FontId::monospace(11.5),
            t.text_muted,
        );
        let scope_galley = p.layout_no_wrap(
            scope.to_string(),
            egui::FontId::proportional(11.5),
            t.text_muted,
        );

        let ext_x = label_x + label_galley.size().x + 8.0;
        let scope_x = (rect.right() - 14.0 - scope_galley.size().x).max(ext_x + 12.0);
        p.galley(
            pos2(label_x, rect.center().y - label_galley.size().y * 0.5),
            label_galley,
            t.text,
        );
        p.galley(
            pos2(ext_x, rect.center().y - ext_galley.size().y * 0.5 + 0.5),
            ext_galley,
            t.text_muted,
        );
        p.galley(
            pos2(scope_x, rect.center().y - scope_galley.size().y * 0.5),
            scope_galley,
            t.text_muted,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(spec.tooltip)
}

fn draw_feedback_banner(ui: &mut egui::Ui, is_error: bool, message: &str, t: &Tokens) {
    let (fill, border, fg, icon) = if is_error {
        (t.danger_bg, t.danger_border, t.danger_fg, Icon::AlertCircle)
    } else {
        (t.toolbar_bg, t.border, t.text_muted, Icon::Info)
    };
    egui::Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                paint_icon(ui.painter(), icon_rect, icon, fg);
                ui.add(egui::Label::new(RichText::new(message).size(11.5).color(fg)).wrap());
            });
        });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{PlatformHooks, modal};
    use drawcraft_doc::UiTheme;
    use drawcraft_engine::Session;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn run_modal_frame(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        events: Vec<egui::Event>,
    ) -> Vec<egui::epaint::ClippedShape> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| {
            crate::theme::apply_visuals(ui.ctx(), app.session.prefs.theme);
            modal::show(app, ui);
        });
        let shapes = out.shapes.clone();
        out.drop_without_applying_deltas();
        shapes
    }

    fn find_text_rects(
        shapes: &[egui::epaint::ClippedShape],
        label: &str,
    ) -> Vec<(egui::Rect, egui::Rect)> {
        fn visit(
            shape: &egui::epaint::Shape,
            clip: egui::Rect,
            label: &str,
            out: &mut Vec<(egui::Rect, egui::Rect)>,
        ) {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                    out.push((text.visual_bounding_rect(), clip));
                }
                egui::epaint::Shape::Vec(items) => {
                    for item in items {
                        visit(item, clip, label, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            visit(&clipped.shape, clipped.clip_rect, label, &mut out);
        }
        out
    }

    fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn export_modal_renders_clean_format_options_and_scope_in_light_and_dark() {
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.select", &json!({ "index": 1 }));
        assert_eq!(app.view.export_format, FileDialogKind::Pdf);

        for theme in [UiTheme::Light, UiTheme::Dark] {
            let theme_name = match theme {
                UiTheme::Light => "Light",
                UiTheme::Dark => "Dark",
            };
            app.session
                .execute("theme.set", &json!({ "theme": theme_name }))
                .unwrap();
            let ctx = egui::Context::default();
            modal::open(&mut app, ActiveModal::Export);
            let mut last_shapes = Vec::new();
            for _ in 0..5 {
                last_shapes = run_modal_frame(&ctx, &mut app, Vec::new());
            }

            for label in [
                "Export Notebook",
                "PDF document",
                ".pdf",
                "All 3 pages",
                "PNG image",
                ".png",
                "Page 2",
                "SVG vector",
                ".svg",
                "Notebook file",
                ".dcraft",
                "Full notebook",
                "Cancel",
                "Export",
            ] {
                let rects = find_text_rects(&last_shapes, label);
                assert!(
                    !rects.is_empty(),
                    "{theme:?} missing expected export text '{label}'"
                );
                for (r, clip) in rects {
                    assert!(
                        r.top() >= clip.top() - 0.5 && r.bottom() <= clip.bottom() + 0.5,
                        "{theme:?} clipped '{label}': {r:?} vs {clip:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn selecting_format_and_clicking_export_saves_and_closes_modal() {
        let captured: Rc<RefCell<Vec<(FileDialogKind, String, usize)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let captured_hook = Rc::clone(&captured);
        let hooks = PlatformHooks {
            save_bytes_dialog: Some(Box::new(move |kind, name, bytes| {
                captured_hook
                    .borrow_mut()
                    .push((kind, name.to_string(), bytes.len()));
            })),
            ..Default::default()
        };
        let mut app = DrawCraftApp::new(Session::default(), hooks);
        let ctx = egui::Context::default();
        modal::open(&mut app, ActiveModal::Export);

        let mut last_shapes = Vec::new();
        for _ in 0..5 {
            last_shapes = run_modal_frame(&ctx, &mut app, Vec::new());
        }

        // Select PNG image row.
        let png_rect = find_text_rects(&last_shapes, "PNG image")[0].0;
        let shapes_after = run_modal_frame(&ctx, &mut app, click_events(png_rect.center()));
        assert_eq!(app.view.export_format, FileDialogKind::Png);

        // Click Export primary button.
        let export_rect = find_text_rects(&shapes_after, "Export")[0].0;
        let _ = run_modal_frame(&ctx, &mut app, click_events(export_rect.center()));
        assert_eq!(app.view.active_modal, None);

        let calls = captured.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, FileDialogKind::Png);
        assert!(calls[0].1.ends_with(".png"));
        assert!(calls[0].2 > 0);
    }

    #[test]
    fn cancelled_save_dialog_keeps_modal_open_with_notice_until_format_changes() {
        let hooks = PlatformHooks {
            pick_save_path: Some(Box::new(|_, _| None)),
            ..Default::default()
        };
        let mut app = DrawCraftApp::new(Session::default(), hooks);
        let ctx = egui::Context::default();
        modal::open(&mut app, ActiveModal::Export);

        let mut last_shapes = Vec::new();
        for _ in 0..5 {
            last_shapes = run_modal_frame(&ctx, &mut app, Vec::new());
        }

        let export_rect = find_text_rects(&last_shapes, "Export")[0].0;
        let _ = run_modal_frame(&ctx, &mut app, click_events(export_rect.center()));
        let shapes = run_modal_frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.view.active_modal, Some(ActiveModal::Export));
        assert!(
            !find_text_rects(&shapes, "Export cancelled.").is_empty(),
            "expected 'Export cancelled.' feedback inside modal"
        );

        // Selecting another format clears the stale cancel notice.
        let svg_rect = find_text_rects(&shapes, "SVG vector")[0].0;
        let shapes_after = run_modal_frame(&ctx, &mut app, click_events(svg_rect.center()));
        assert_eq!(app.view.export_format, FileDialogKind::Svg);
        assert!(find_text_rects(&shapes_after, "Export cancelled.").is_empty());

        // Clicking Cancel closes without triggering export.
        let cancel_rect = find_text_rects(&shapes_after, "Cancel")[0].0;
        let _ = run_modal_frame(&ctx, &mut app, click_events(cancel_rect.center()));
        assert_eq!(app.view.active_modal, None);
    }

    #[test]
    fn trigger_export_selected_pages_pdf_exports_only_requested_pages() {
        let captured: Rc<RefCell<Vec<(FileDialogKind, String, usize)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let captured_hook = Rc::clone(&captured);
        let hooks = PlatformHooks {
            save_bytes_dialog: Some(Box::new(move |kind, name, bytes| {
                captured_hook
                    .borrow_mut()
                    .push((kind, name.to_string(), bytes.len()));
            })),
            ..Default::default()
        };
        let mut app = DrawCraftApp::new(Session::default(), hooks);
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.add", &json!({}));

        trigger_export_selected_pages_pdf(&mut app, &[0, 2]);
        let calls = captured.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, FileDialogKind::Pdf);
        assert!(calls[0].1.ends_with("_selected.pdf"));
        assert!(calls[0].2 > 0);
        assert_eq!(
            app.session.last_export_status.as_deref(),
            Some(format!("Prepared 2-page PDF ({} bytes).", calls[0].2).as_str())
        );
    }

    #[test]
    fn export_modal_supports_single_page_combined_ordered_and_separate_files_exports() {
        type CapturedBytes = Rc<RefCell<Vec<(FileDialogKind, String, Vec<u8>)>>>;
        let captured: CapturedBytes = Rc::new(RefCell::new(Vec::new()));
        let captured_hook = Rc::clone(&captured);
        let hooks = PlatformHooks {
            save_bytes_dialog: Some(Box::new(move |kind, name, bytes| {
                captured_hook
                    .borrow_mut()
                    .push((kind, name.to_string(), bytes));
            })),
            ..Default::default()
        };
        let mut app = DrawCraftApp::new(Session::default(), hooks);
        // Create 3 distinct paper colors across pages 1..3 to verify exact page selection & ordering.
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));
        let _ = app
            .session
            .execute("page.setTemplate", &json!({ "color": "WarmCream" }));
        let _ = app.session.execute("page.select", &json!({ "index": 2 }));
        let _ = app
            .session
            .execute("page.setTemplate", &json!({ "color": "DarkSlate" }));

        let ctx = egui::Context::default();

        // 1. Single page export from right-click ("Page 3").
        open_for_pages(&mut app, vec![2]);
        let mut shapes = Vec::new();
        for _ in 0..5 {
            shapes = run_modal_frame(&ctx, &mut app, Vec::new());
        }
        assert!(!find_text_rects(&shapes, "Selected page: Page 3").is_empty());
        let export_rect = find_text_rects(&shapes, "Export")[0].0;
        let _ = run_modal_frame(&ctx, &mut app, click_events(export_rect.center()));
        assert_eq!(app.view.active_modal, None);
        assert!(
            app.session
                .last_export_status
                .as_deref()
                .is_some_and(|s| s.starts_with("Prepared 1-page PDF")),
            "status={:?}",
            app.session.last_export_status
        );
        assert_eq!(captured.borrow().len(), 1);
        captured.borrow_mut().clear();

        // 2. Multi-page selection [2, 0] combined together into .dcraft preserving order Page 3 -> Page 1.
        open_for_pages(&mut app, vec![2, 0]);
        for _ in 0..5 {
            shapes = run_modal_frame(&ctx, &mut app, Vec::new());
        }
        assert!(!find_text_rects(&shapes, "Selection order (2): Page 3 → Page 1").is_empty());
        assert!(!find_text_rects(&shapes, "2 selected pages").is_empty());
        let ncraft_rect = find_text_rects(&shapes, "Notebook file")[0].0;
        let shapes_ncraft = run_modal_frame(&ctx, &mut app, click_events(ncraft_rect.center()));
        let export_rect = find_text_rects(&shapes_ncraft, "Export")[0].0;
        let _ = run_modal_frame(&ctx, &mut app, click_events(export_rect.center()));
        {
            let calls = captured.borrow();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, FileDialogKind::Dcraft);
            let mut probe = Session::default();
            probe.load_notebook_bytes(&calls[0].2, None, true).unwrap();
            assert_eq!(probe.doc.page_count(), 2);
            assert_eq!(
                probe.doc.pages[0].paper_color,
                drawcraft_doc::PaperColor::DarkSlate
            );
            assert_eq!(
                probe.doc.pages[1].paper_color,
                drawcraft_doc::PaperColor::WarmCream
            );
        }
        captured.borrow_mut().clear();

        // 3. Toggle "Export pages as separate files" checkbox -> saves 2 files in order Page 3 then Page 1.
        app.view.export_format = FileDialogKind::Pdf;
        open_for_pages(&mut app, vec![2, 0]);
        for _ in 0..5 {
            shapes = run_modal_frame(&ctx, &mut app, Vec::new());
        }
        let check_rect = find_text_rects(&shapes, "Export pages as separate files")[0].0;
        let shapes_checked = run_modal_frame(&ctx, &mut app, click_events(check_rect.center()));
        assert!(app.view.export_separate_files);
        assert!(!find_text_rects(&shapes_checked, "2 separate files").is_empty());
        let export_rect = find_text_rects(&shapes_checked, "Export")[0].0;
        let _ = run_modal_frame(&ctx, &mut app, click_events(export_rect.center()));
        let calls = captured.borrow();
        assert_eq!(calls.len(), 2);
        assert!(
            calls[0].1.ends_with("_01_page3.pdf") && !calls[0].2.is_empty(),
            "first separate file name={}",
            calls[0].1
        );
        assert!(
            calls[1].1.ends_with("_02_page1.pdf") && !calls[1].2.is_empty(),
            "second separate file name={}",
            calls[1].1
        );
    }
}
