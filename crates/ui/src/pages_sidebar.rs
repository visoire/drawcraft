//! Left sidebar showing clean 2-column visual page thumbnails, inline page
//! navigation, and quick active-page actions.

use std::sync::Arc;

use drawcraft_doc::{InkStroke, NotebookPage, PenKind};
use egui::Color32;
use serde_json::json;

use crate::DrawCraftApp;
use crate::customization_modals::paint_paper_preview_colors;
use crate::icons::{Icon, compact_icon_button, icon_button, paint_icon};
use crate::inspector::card_frame;
use crate::modal::{self, ActiveModal};
use crate::theme::{Tokens, to_color32};

pub(crate) const DEFAULT_SIDEBAR_WIDTH: f32 = 276.0;
const COLLAPSED_FOOTER_HEIGHT_ESTIMATE: f32 = 80.0;
const EXPANDED_DRAWER_HEIGHT_ESTIMATE: f32 = 121.0;
const BATCH_FOOTER_HEIGHT_ESTIMATE: f32 = 163.0;

/// Renders the left-hand Pages sidebar.
pub fn show(app: &mut DrawCraftApp, ui: &mut egui::Ui) {
    let _ = show_inner(app, ui);
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SidebarMetrics {
    pub(crate) panel_rect: egui::Rect,
    pages_header_rect: egui::Rect,
    scroll_rect: egui::Rect,
    first_tile_rect: egui::Rect,
    active_tile_rect: egui::Rect,
    first_bookmark_rect: egui::Rect,
    footer_rect: egui::Rect,
    prev_btn_rect: egui::Rect,
    next_btn_rect: egui::Rect,
    footer_toggle_rect: egui::Rect,
}

impl Default for SidebarMetrics {
    fn default() -> Self {
        Self {
            panel_rect: egui::Rect::NOTHING,
            pages_header_rect: egui::Rect::NOTHING,
            scroll_rect: egui::Rect::NOTHING,
            first_tile_rect: egui::Rect::NOTHING,
            active_tile_rect: egui::Rect::NOTHING,
            first_bookmark_rect: egui::Rect::NOTHING,
            footer_rect: egui::Rect::NOTHING,
            prev_btn_rect: egui::Rect::NOTHING,
            next_btn_rect: egui::Rect::NOTHING,
            footer_toggle_rect: egui::Rect::NOTHING,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FooterLayout {
    footer_rect: egui::Rect,
    prev_rect: egui::Rect,
    next_rect: egui::Rect,
    toggle_rect: egui::Rect,
    measured_drawer_h: Option<f32>,
    cur_drawer_h: f32,
}

#[derive(Clone, Copy, Debug)]
struct GridTiles {
    first_tile_rect: egui::Rect,
    active_tile_rect: egui::Rect,
    first_bookmark_rect: egui::Rect,
    layout_settled: bool,
    drag_drop_target: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct TileDrawResult {
    tile_rect: egui::Rect,
    bookmark_rect: egui::Rect,
}

struct TileFooterResult {
    sub_action_taken: bool,
    bookmark_rect: egui::Rect,
    label_resp: Option<egui::Response>,
}

pub(crate) fn show_inner(app: &mut DrawCraftApp, ui: &mut egui::Ui) -> SidebarMetrics {
    let t = Tokens::for_theme(app.session.prefs.theme);
    let mut metrics = SidebarMetrics::default();

    let prev_open = app.view.pages_sidebar_open;
    let mut panel_open = prev_open;
    let mut drag_drop_target: Option<usize> = None;
    let reveal_id = ui.make_persistent_id("drawcraft_pages_sidebar_reveal");
    let reveal = crate::motion::panel_reveal_factor(ui.ctx(), reveal_id, panel_open);
    let content_opacity = crate::motion::panel_content_opacity(reveal);

    let frame = egui::Frame::NONE
        .fill(t.panel_bg)
        .inner_margin(egui::Margin::same(12));
    if let Some(panel_resp) = crate::fixed_side_panel(
        ui,
        if app.session.prefs.left_handed {
            egui::Panel::right("drawcraft_pages_sidebar")
        } else {
            egui::Panel::left("drawcraft_pages_sidebar")
        }
        .drag_to_open(false),
        DEFAULT_SIDEBAR_WIDTH,
        frame,
    )
    .show_collapsible(ui, &mut panel_open, |ui| {
        ui.multiply_opacity(content_opacity);
        metrics.pages_header_rect = draw_pages_header(app, ui, &t);
        ui.add_space(6.0);

        let page_count = app.session.doc.page_count();
        let active_idx = app
            .session
            .doc
            .active_page_index()
            .min(page_count.saturating_sub(1));
        let active_page = app.session.doc.with_resolved_page(active_idx, Clone::clone);

        // Lay out the scroll viewport and docked footer top-down so the
        // footer stays pinned to the bottom at its exact natural height
        // without colliding with the page thumbnail grid.
        let footer_gap = 8.0_f32;
        let item_gap = ui.spacing().item_spacing.y;
        let expand_id = ui.make_persistent_id("drawcraft_pages_footer_expand");
        let expand_t =
            crate::motion::panel_reveal_factor(ui.ctx(), expand_id, app.view.pages_footer_expanded);
        let collapsed_h_id = ui.make_persistent_id("drawcraft_pages_footer_collapsed_h");
        let drawer_h_id = ui.make_persistent_id("drawcraft_pages_footer_drawer_h");
        let batch_h_id = ui.make_persistent_id("drawcraft_pages_batch_footer_h");
        let cached_collapsed_h = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(collapsed_h_id))
            .unwrap_or(COLLAPSED_FOOTER_HEIGHT_ESTIMATE);
        let cached_drawer_h = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(drawer_h_id))
            .unwrap_or(EXPANDED_DRAWER_HEIGHT_ESTIMATE);
        let cached_batch_h = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(batch_h_id))
            .unwrap_or(BATCH_FOOTER_HEIGHT_ESTIMATE);
        let expected_footer_h = if app.view.pages_select_mode {
            cached_batch_h
        } else {
            cached_collapsed_h + cached_drawer_h * expand_t
        };
        let avail_h = ui.available_height();
        let scroll_h = (avail_h - expected_footer_h - footer_gap - item_gap).max(48.0);

        let current_page_state = (app.session.active_notebook, active_idx, page_count);
        let last_page_state_id = ui.make_persistent_id("drawcraft_pages_sidebar_last_page_state");
        let last_page_state = ui
            .ctx()
            .data(|d| d.get_temp::<(usize, usize, usize)>(last_page_state_id));
        let scroll_to_active = last_page_state != Some(current_page_state);
        let outer_settled_id = ui.make_persistent_id("drawcraft_pages_outer_settled");
        let prev_outer_settled = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(outer_settled_id))
            .unwrap_or(false);

        // Keep selected_page_indices valid after removals/notebook changes.
        app.view
            .selected_page_indices
            .retain(|&idx| idx < page_count);

        let visible_indices: Vec<usize> = if app.view.pages_bookmarked_only {
            app.session.doc.bookmarked_page_indices()
        } else {
            (0..page_count).collect()
        };

        let scroll_out = egui::ScrollArea::vertical()
            .id_salt("pages_thumbnail_scroll")
            .max_height(scroll_h)
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let tiles = draw_thumbnail_grid(
                    app,
                    ui,
                    page_count,
                    &visible_indices,
                    active_idx,
                    viewport,
                    &t,
                );
                if scroll_to_active
                    && prev_outer_settled
                    && tiles.layout_settled
                    && tiles.active_tile_rect != egui::Rect::NOTHING
                {
                    ui.scroll_to_rect(tiles.active_tile_rect, None);
                }
                tiles
            });
        metrics.scroll_rect = scroll_out.inner_rect;
        metrics.first_tile_rect = scroll_out.inner.first_tile_rect;
        metrics.active_tile_rect = scroll_out.inner.active_tile_rect;
        metrics.first_bookmark_rect = scroll_out.inner.first_bookmark_rect;
        drag_drop_target = scroll_out.inner.drag_drop_target;

        ui.add_space(footer_gap);
        let footer_layout = if app.view.pages_select_mode {
            draw_batch_selection_footer(app, ui, page_count, &visible_indices, &t)
        } else {
            draw_active_page_footer(
                app,
                ui,
                ActivePageFooterCtx {
                    active_idx,
                    page_count,
                    active_page: active_page.as_ref(),
                    expand_t,
                    natural_drawer_h: cached_drawer_h,
                },
                &t,
            )
        };
        metrics.footer_rect = footer_layout.footer_rect;
        metrics.prev_btn_rect = footer_layout.prev_rect;
        metrics.next_btn_rect = footer_layout.next_rect;
        metrics.footer_toggle_rect = footer_layout.toggle_rect;
        let measured_footer_h = metrics.footer_rect.height();
        let mut footer_settled = (measured_footer_h - expected_footer_h).abs() <= 0.5;
        if app.view.pages_select_mode {
            if !footer_settled {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(batch_h_id, measured_footer_h));
                ui.ctx().request_repaint();
            }
        } else {
            let measured_collapsed_h = (measured_footer_h - footer_layout.cur_drawer_h).max(40.0);
            if (measured_collapsed_h - cached_collapsed_h).abs() > 0.5 {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(collapsed_h_id, measured_collapsed_h));
                footer_settled = false;
                ui.ctx().request_repaint();
            }
            if let Some(measured_drawer_h) = footer_layout.measured_drawer_h
                && (measured_drawer_h - cached_drawer_h).abs() > 0.5
            {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(drawer_h_id, measured_drawer_h));
                footer_settled = false;
                ui.ctx().request_repaint();
            }
        }
        let outer_settled = footer_settled;
        ui.ctx()
            .data_mut(|d| d.insert_temp(outer_settled_id, outer_settled));
        let all_settled = prev_outer_settled && outer_settled && scroll_out.inner.layout_settled;
        if scroll_to_active {
            if all_settled {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(last_page_state_id, current_page_state));
            } else {
                ui.ctx().request_repaint();
            }
        }
    }) {
        metrics.panel_rect = panel_resp.response.rect;
        let edge_x = if app.session.prefs.left_handed {
            metrics.panel_rect.left()
        } else {
            metrics.panel_rect.right()
        };
        ui.painter().line_segment(
            [
                egui::pos2(edge_x, metrics.panel_rect.top()),
                egui::pos2(edge_x, metrics.panel_rect.bottom()),
            ],
            egui::Stroke::new(1.0, t.border),
        );
    }
    if panel_open != prev_open {
        app.view.pages_sidebar_open = panel_open;
    }
    if !panel_open
        || app.view.active_modal.is_some()
        || app.view.pages_select_mode
        || app.view.pages_bookmarked_only
        || app.session.doc.page_count() <= 1
    {
        app.view.pages_drag_source = None;
    } else if let Some(src) = app.view.pages_drag_source {
        paint_page_drag_ghost(app, ui.ctx(), src, drag_drop_target, &t);
    }
    metrics
}

fn draw_pages_header(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> egui::Rect {
    let bookmark_count = app.session.doc.bookmarked_page_count();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.horizontal(|ui| {
            let row_h = crate::icons::min_target(ui.ctx(), 28.0);
            ui.set_height(row_h);
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                egui::RichText::new("Pages")
                    .family(egui::FontFamily::Name(crate::theme::DISPLAY_FAMILY.into()))
                    .size(16.0)
                    .color(t.text),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let nb_settings_open = app.view.active_modal == Some(ActiveModal::NotebookSettings);
                if compact_icon_button(
                    ui,
                    Icon::Settings,
                    Some("Notebook Settings"),
                    nb_settings_open,
                    true,
                    t,
                )
                .on_hover_text(
                    "Customize notebook details, cover, tags, page size, and paper style",
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
                {
                    modal::open(app, ActiveModal::NotebookSettings);
                }
            });
        });

        ui.add_space(8.0);

        ui.spacing_mut().item_spacing.x = 6.0;
        let pill_h = crate::icons::min_target(ui.ctx(), 30.0);
        ui.columns(2, |cols| {
            let bm_label = if bookmark_count > 0 {
                format!("Saved ({bookmark_count})")
            } else {
                "Saved".to_string()
            };
            let bm_tone = if app.view.pages_bookmarked_only {
                ActionTone::Selected
            } else {
                ActionTone::Surface
            };
            if sidebar_action_button(
                &mut cols[0],
                Icon::Bookmark,
                &bm_label,
                bm_tone,
                true,
                pill_h,
                t,
            )
            .on_hover_text("Show only bookmarked pages in this notebook")
            .clicked()
            {
                app.view.pages_bookmarked_only = !app.view.pages_bookmarked_only;
            }

            let (select_icon, select_label, select_tone, select_tip) = if app.view.pages_select_mode
            {
                (
                    Icon::Check,
                    "Done",
                    ActionTone::Selected,
                    "Finish selecting pages and return to active page actions",
                )
            } else {
                (
                    Icon::CheckSquare,
                    "Select",
                    ActionTone::Surface,
                    "Select multiple pages (or Shift+click any page) to duplicate, rotate, export, transfer, or remove",
                )
            };
            if sidebar_action_button(
                &mut cols[1],
                select_icon,
                select_label,
                select_tone,
                true,
                pill_h,
                t,
            )
            .on_hover_text(select_tip)
            .clicked()
            {
                app.view.pages_select_mode = !app.view.pages_select_mode;
                if !app.view.pages_select_mode {
                    app.view.selected_page_indices.clear();
                }
            }
        });

        ui.add_space(8.0);
        draw_card_hairline(ui, t);
    })
    .response
    .rect
}

fn tile_sheet_height(doc: &drawcraft_doc::Document, page_idx: usize, sheet_w: f32) -> f32 {
    let (w, h) = doc.page_dimensions(page_idx).unwrap_or((1200, 1600));
    let aspect = (h.max(1) as f32) / (w.max(1) as f32);
    (sheet_w * aspect).clamp(64.0, 168.0)
}

fn compute_grid_row_metrics(
    doc: &drawcraft_doc::Document,
    visible_indices: &[usize],
    sheet_w: f32,
    chrome_h: f32,
    row_gap: f32,
) -> (Vec<f32>, Vec<f32>, f32) {
    let total_rows = visible_indices.len().div_ceil(2);
    let mut row_tops = Vec::with_capacity(total_rows);
    let mut row_heights = Vec::with_capacity(total_rows);
    let mut y = 0.0_f32;
    for pair in visible_indices.chunks(2) {
        let max_sheet_h = pair
            .iter()
            .map(|&idx| tile_sheet_height(doc, idx, sheet_w))
            .fold(64.0_f32, f32::max);
        let row_h = max_sheet_h + chrome_h;
        row_tops.push(y);
        row_heights.push(row_h);
        y += row_h + row_gap;
    }
    let total_h = if total_rows == 0 { 0.0 } else { y - row_gap };
    (row_tops, row_heights, total_h)
}

fn visible_row_bounds(
    row_tops: &[f32],
    row_heights: &[f32],
    viewport: egui::Rect,
) -> (usize, usize) {
    let total_rows = row_tops.len();
    if total_rows == 0 {
        return (0, 0);
    }
    let vis_top = (viewport.top() - 80.0).max(0.0);
    let vis_bot = viewport.bottom() + 80.0;
    let mut first = row_tops.partition_point(|&top| top + 256.0 < vis_top);
    while first < total_rows && row_tops[first] + row_heights[first] < vis_top {
        first += 1;
    }
    if first >= total_rows {
        let last = total_rows - 1;
        return (last, last);
    }
    let mut last = first;
    while last + 1 < total_rows && row_tops[last + 1] <= vis_bot {
        last += 1;
    }
    (first, last)
}

fn grid_chrome_h_id(ui: &egui::Ui) -> egui::Id {
    ui.make_persistent_id("drawcraft_pages_tile_chrome_h")
}

fn update_grid_layout_settled(
    ui: &egui::Ui,
    avail_w: f32,
    bar_expected: bool,
    measured_chrome_h: Option<f32>,
) -> bool {
    let avail_w_id = ui.make_persistent_id("drawcraft_pages_grid_avail_w");
    let prev_avail_w = ui.ctx().data(|d| d.get_temp::<f32>(avail_w_id));
    let avail_w_settled = prev_avail_w.is_some_and(|w| (w - avail_w).abs() <= 0.05);
    if !avail_w_settled {
        ui.ctx().data_mut(|d| d.insert_temp(avail_w_id, avail_w));
    }

    let bar_id = ui.make_persistent_id("drawcraft_pages_grid_bar_expected");
    let prev_bar = ui.ctx().data(|d| d.get_temp::<bool>(bar_id));
    let bar_settled = prev_bar == Some(bar_expected);
    if !bar_settled {
        ui.ctx().data_mut(|d| d.insert_temp(bar_id, bar_expected));
    }

    let chrome_id = grid_chrome_h_id(ui);
    let cached_chrome = ui.ctx().data(|d| d.get_temp::<f32>(chrome_id));
    let chrome_ref = cached_chrome.unwrap_or(40.0);
    let chrome_settled =
        cached_chrome.is_some() && measured_chrome_h.is_none_or(|m| (m - chrome_ref).abs() <= 0.05);
    if let Some(measured) = measured_chrome_h
        && (cached_chrome.is_none() || (measured - chrome_ref).abs() > 0.05)
    {
        ui.ctx().data_mut(|d| d.insert_temp(chrome_id, measured));
    }

    let settled = avail_w_settled && bar_settled && chrome_settled;
    if !settled {
        ui.ctx().request_repaint();
    }
    settled
}

fn draw_thumbnail_grid(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    total_pages: usize,
    visible_indices: &[usize],
    active_idx: usize,
    viewport: egui::Rect,
    t: &Tokens,
) -> GridTiles {
    let mut tiles = GridTiles {
        first_tile_rect: egui::Rect::NOTHING,
        active_tile_rect: egui::Rect::NOTHING,
        first_bookmark_rect: egui::Rect::NOTHING,
        layout_settled: true,
        drag_drop_target: None,
    };
    if visible_indices.is_empty() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No bookmarked pages yet")
                    .size(12.5)
                    .strong()
                    .color(t.text),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Click the bookmark icon on any page thumbnail to pin it here.",
                )
                .size(11.0)
                .color(t.text_muted),
            );
        });
        return tiles;
    }

    let avail_w = ui.available_width();
    let col_gap = ui.spacing().item_spacing.x;
    let item_spacing_y = ui.spacing().item_spacing.y;
    let col_w = ((avail_w - col_gap).max(104.0)) * 0.5;
    let sheet_w = (col_w - 14.0).clamp(52.0, 128.0);
    let row_gap = 6.0 + item_spacing_y;

    let cached_chrome_h = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(grid_chrome_h_id(ui)))
        .unwrap_or(40.0);

    let (row_tops, row_heights, total_h) = compute_grid_row_metrics(
        &app.session.doc,
        visible_indices,
        sheet_w,
        cached_chrome_h,
        row_gap,
    );
    let total_rows = row_tops.len();
    let (first_vis, last_vis) = visible_row_bounds(&row_tops, &row_heights, viewport);
    let content_origin = ui.cursor().left_top();

    if first_vis > 0 {
        let top_skip = (row_tops[first_vis] - item_spacing_y).max(0.0);
        ui.allocate_space(egui::vec2(avail_w, top_skip));
    }

    let mut interaction = GridInteractionState {
        pending_move: None,
        hovered_tile: None,
        pointer_released: ui.input(|i| i.pointer.any_released()),
    };
    let pointer_down = ui.input(|i| i.pointer.any_down());
    let escape_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let mut measured_chrome_h: Option<f32> = None;

    for row_idx in first_vis..=last_vis {
        if row_idx > first_vis {
            ui.add_space(6.0);
        }
        let start = row_idx * 2;
        let end = (start + 2).min(visible_indices.len());
        let pair = &visible_indices[start..end];
        ui.columns(2, |cols| {
            for (col_idx, &page_idx) in pair.iter().enumerate() {
                let Some(page) = app.session.doc.with_resolved_page(page_idx, Clone::clone) else {
                    continue;
                };
                let tile = draw_page_tile(
                    app,
                    &mut cols[col_idx],
                    TileRenderArgs {
                        page: &page,
                        page_idx,
                        total_pages,
                        selected: page_idx == active_idx,
                    },
                    &mut interaction,
                    t,
                );
                let tile_rect = tile.tile_rect;
                if measured_chrome_h.is_none() {
                    let tile_sheet_h = tile_sheet_height(&app.session.doc, page_idx, sheet_w);
                    measured_chrome_h = Some((tile_rect.height() - tile_sheet_h).max(20.0));
                }
                if row_idx == 0 && col_idx == 0 {
                    tiles.first_tile_rect = tile_rect;
                    tiles.first_bookmark_rect = tile.bookmark_rect;
                }
                if page_idx == active_idx {
                    tiles.active_tile_rect = tile_rect;
                }
            }
        });
    }

    if last_vis + 1 < total_rows {
        let cursor_rel = ui.cursor().top() - content_origin.y;
        let bottom_skip = (total_h - cursor_rel).max(0.0);
        if bottom_skip > 0.0 {
            ui.allocate_space(egui::vec2(avail_w, bottom_skip));
        }
    } else if !app.view.pages_bookmarked_only && !app.view.pages_select_mode {
        // Calm studio-style `+ Add Page` tile tucked right after the last thumbnail row.
        ui.add_space(8.0);
        let btn_h = crate::icons::min_target(ui.ctx(), 38.0);
        let (add_rect, add_resp) =
            ui.allocate_exact_size(egui::vec2(avail_w, btn_h), egui::Sense::click());
        if ui.is_rect_visible(add_rect) {
            let hov =
                crate::motion::hover_factor(ui.ctx(), add_resp.id.with("hov"), add_resp.hovered());
            let fill = crate::motion::lerp_color(t.field_bg, t.accent_soft, hov);
            let border = crate::motion::lerp_color(t.border, t.accent, hov);
            let fg = crate::motion::lerp_color(t.text_muted, t.accent, hov);
            let radius = egui::CornerRadius::same(crate::theme::CONTROL_RADIUS);
            let p = ui.painter();
            p.rect_filled(add_rect, radius, fill);
            p.rect_stroke(
                add_rect,
                radius,
                egui::Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            );
            let galley = p.layout_no_wrap(
                "Add Page".to_string(),
                egui::FontId::new(
                    12.5,
                    egui::FontFamily::Name(crate::theme::MEDIUM_FAMILY.into()),
                ),
                fg,
            );
            let icon_dim = 14.0_f32;
            let total_w = icon_dim + 6.0 + galley.size().x;
            let start_x = add_rect.center().x - total_w * 0.5;
            let ir = egui::Rect::from_center_size(
                egui::pos2(start_x + icon_dim * 0.5, add_rect.center().y),
                egui::vec2(icon_dim, icon_dim),
            );
            paint_icon(p, ir, Icon::Plus, fg);
            p.galley(
                egui::pos2(
                    ir.right() + 6.0,
                    add_rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                fg,
            );
        }
        if add_resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Add a new page to this notebook")
            .clicked()
        {
            crate::canvas::commit_pending_text(app);
            let _ = app.session.execute("page.add", &json!({}));
            app.mesh_cache.clear();
            app.view.fit_requested = true;
            ui.ctx().request_repaint();
        }
    }

    let bar_expected = total_h > viewport.height() + 0.5;
    tiles.layout_settled = update_grid_layout_settled(ui, avail_w, bar_expected, measured_chrome_h);

    if tiles.first_tile_rect == egui::Rect::NOTHING && total_rows > 0 {
        tiles.first_tile_rect =
            egui::Rect::from_min_size(content_origin, egui::vec2(col_w, row_heights[0]));
    }
    if tiles.active_tile_rect == egui::Rect::NOTHING
        && let Some(vis_pos) = visible_indices.iter().position(|&idx| idx == active_idx)
    {
        let r = vis_pos / 2;
        let c = vis_pos % 2;
        let x = content_origin.x + (c as f32) * (col_w + col_gap);
        let y = content_origin.y + row_tops[r];
        tiles.active_tile_rect =
            egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(col_w, row_heights[r]));
    }

    if escape_pressed {
        interaction.pending_move = None;
        app.view.pages_drag_source = None;
    }
    commit_or_prompt_page_move(app, interaction.pending_move, pointer_down, ui);
    tiles.drag_drop_target = app
        .view
        .pages_drag_source
        .and_then(|src| interaction.hovered_tile.filter(|&dst| dst != src));
    tiles
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingPageMove {
    from: usize,
    to: usize,
}

struct GridInteractionState {
    pending_move: Option<PendingPageMove>,
    hovered_tile: Option<usize>,
    pointer_released: bool,
}

struct TileRenderArgs<'a> {
    page: &'a NotebookPage,
    page_idx: usize,
    total_pages: usize,
    selected: bool,
}

struct SheetPreviewOpts {
    page_idx: usize,
    border_color: Color32,
    stroke_cache_scope: &'static str,
}

fn commit_or_prompt_page_move(
    app: &mut DrawCraftApp,
    pending_move: Option<PendingPageMove>,
    pointer_down: bool,
    ui: &egui::Ui,
) {
    match pending_move {
        Some(PendingPageMove { from, to }) => {
            app.view.pages_drag_source = None;
            app.view.pending_page_reorder = Some(crate::PendingPageReorder {
                notebook_index: app.session.active_notebook,
                from,
                to,
            });
            modal::open(app, ActiveModal::ConfirmReorderPage);
            ui.ctx().request_repaint();
        }
        None => {
            if !pointer_down {
                app.view.pages_drag_source = None;
            }
        }
    }
}

/// Renders the miniature paper, backdrop, images, strokes, text, and shapes inside `sheet_rect`.
fn paint_page_sheet_contents(
    app: &DrawCraftApp,
    ui: &mut egui::Ui,
    sheet_rect: egui::Rect,
    page: &NotebookPage,
    opts: SheetPreviewOpts,
) {
    let theme = app.session.prefs.theme;
    let (auto_contrast, eff_paper_color) =
        crate::effective_page_ink_context(page, &app.session.prefs);
    let paper_colors = eff_paper_color.resolved_colors(theme, auto_contrast);
    let has_backdrop = page.pdf_backdrop.is_some() || page.custom_image_backdrop.is_some();
    let eff_tpl = if has_backdrop {
        drawcraft_doc::PaperTemplate::Blank
    } else {
        page.paper_template
    };
    paint_paper_preview_colors(
        ui.painter(),
        sheet_rect,
        eff_tpl,
        paper_colors,
        opts.border_color,
    );
    if has_backdrop {
        crate::pdf_view::paint_page_pdf_backdrop_if_present(
            ui.ctx(),
            ui.painter(),
            sheet_rect,
            &app.session.doc,
            page,
            240,
        );
    }
    paint_thumbnail_images(ui, sheet_rect, page);
    paint_thumbnail_foreground_cached(
        ui,
        sheet_rect,
        ThumbForegroundTarget {
            page,
            notebook_idx: app.session.active_notebook,
            page_idx: opts.page_idx,
            scope: opts.stroke_cache_scope,
            theme,
            auto_contrast,
            defer_rebuild: app.view.selection_drag.is_some(),
        },
    );
}

fn paint_page_tile_sheet(
    app: &DrawCraftApp,
    ui: &mut egui::Ui,
    page: &NotebookPage,
    page_idx: usize,
    border_color: Color32,
    batch_selected: bool,
    t: &Tokens,
) {
    let avail_w = ui.available_width().clamp(52.0, 128.0);
    let aspect = (page.height.max(1) as f32) / (page.width.max(1) as f32);
    let sheet_w = avail_w;
    let sheet_h = if aspect.is_finite() {
        (sheet_w * aspect).clamp(64.0, 168.0)
    } else {
        112.0
    };
    let (sheet_rect, _) =
        ui.allocate_exact_size(egui::vec2(sheet_w, sheet_h), egui::Sense::empty());

    if ui.is_rect_visible(sheet_rect) {
        paint_page_sheet_contents(
            app,
            ui,
            sheet_rect,
            page,
            SheetPreviewOpts {
                page_idx,
                border_color,
                stroke_cache_scope: "tile",
            },
        );
    }

    if app.view.pages_select_mode {
        let check_rect = egui::Rect::from_min_size(
            sheet_rect.left_top() + egui::vec2(4.0, 4.0),
            egui::vec2(16.0, 16.0),
        );
        ui.painter().rect_filled(
            check_rect,
            4.0,
            if batch_selected {
                t.accent
            } else {
                t.panel_bg.gamma_multiply(0.9)
            },
        );
        ui.painter().rect_stroke(
            check_rect,
            4.0,
            egui::Stroke::new(1.0, if batch_selected { t.accent } else { t.border }),
            egui::StrokeKind::Inside,
        );
        if batch_selected {
            paint_icon(
                ui.painter(),
                check_rect.shrink(2.5),
                Icon::Check,
                Color32::WHITE,
            );
        }
    }
}

fn clamp_ghost_pos(raw_pos: egui::Pos2, est_size: egui::Vec2, screen: egui::Rect) -> egui::Pos2 {
    if !screen.is_finite() || screen.width() <= 0.0 || screen.height() <= 0.0 {
        return raw_pos;
    }
    let min_x = screen.left();
    let max_x = (screen.right() - est_size.x.max(0.0)).max(min_x);
    let min_y = screen.top();
    let max_y = (screen.bottom() - est_size.y.max(0.0)).max(min_y);
    egui::pos2(raw_pos.x.clamp(min_x, max_x), raw_pos.y.clamp(min_y, max_y))
}

fn draw_drag_ghost_caption(
    ui: &mut egui::Ui,
    src_idx: usize,
    drop_target: Option<usize>,
    max_w: f32,
    t: &Tokens,
) {
    ui.set_max_width(max_w);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::empty());
        paint_icon(ui.painter(), icon_rect, Icon::Paper, t.accent);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("Page {}", src_idx + 1))
                    .size(11.5)
                    .strong()
                    .color(t.text),
            )
            .truncate(),
        );
    });
    let (hint_text, hint_color) = if let Some(dst) = drop_target {
        (format!("Move to Pos {}", dst + 1), t.accent)
    } else {
        ("Drop on page".to_string(), t.text_muted)
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(hint_text)
                .size(10.0)
                .strong()
                .color(hint_color),
        )
        .truncate(),
    );
}

/// Floating miniature page card that follows the pointer while reordering pages.
fn paint_page_drag_ghost(
    app: &DrawCraftApp,
    ctx: &egui::Context,
    src_idx: usize,
    drop_target: Option<usize>,
    t: &Tokens,
) {
    let Some(pos) = ctx.input(|i| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()))
    else {
        return;
    };
    if !pos.x.is_finite() || !pos.y.is_finite() {
        return;
    }
    let Some(page) = app.session.doc.with_resolved_page(src_idx, Clone::clone) else {
        return;
    };
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);

    let aspect = (page.height.max(1) as f32) / (page.width.max(1) as f32);
    let sheet_w = 88.0_f32;
    let sheet_h = if aspect.is_finite() {
        (sheet_w * aspect).clamp(54.0, 124.0)
    } else {
        117.0
    };
    let ghost_id = egui::Id::unique("drawcraft_page_drag_ghost");
    let size_id = ghost_id.with("last_size");
    let est_size = ctx
        .data(|d| d.get_temp::<egui::Vec2>(size_id))
        .unwrap_or_else(|| egui::vec2(sheet_w + 16.0, sheet_h + 48.0));
    let screen_rect = ctx.content_rect();
    let clamped_pos = clamp_ghost_pos(
        pos + egui::vec2(14.0, 12.0),
        est_size,
        screen_rect.shrink(8.0),
    );

    // Paint directly on a non-interactive Tooltip layer instead of `egui::Area`
    // so there is no invisible 1-frame sizing pass when dragging starts.
    let layer_id = egui::LayerId::new(egui::Order::Tooltip, ghost_id);
    let max_rect =
        egui::Rect::from_min_size(clamped_pos, egui::vec2(sheet_w + 24.0, sheet_h + 80.0));
    let mut ghost_ui = egui::Ui::new(
        ctx.clone(),
        ghost_id,
        egui::UiBuilder::new()
            .layer_id(layer_id)
            .max_rect(max_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ghost_ui.set_clip_rect(screen_rect);
    ghost_ui.multiply_opacity(0.94);
    let frame_resp = egui::Frame::NONE
        .fill(t.field_bg)
        .stroke(egui::Stroke::new(1.5, t.accent))
        .corner_radius(egui::CornerRadius::same(9))
        .inner_margin(egui::Margin::same(6))
        .shadow(ctx.style_of(ctx.theme()).visuals.popup_shadow)
        .show(&mut ghost_ui, |ui| {
            ui.set_width(sheet_w);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                let (sheet_rect, _) =
                    ui.allocate_exact_size(egui::vec2(sheet_w, sheet_h), egui::Sense::empty());
                paint_page_sheet_contents(
                    app,
                    ui,
                    sheet_rect,
                    &page,
                    SheetPreviewOpts {
                        page_idx: src_idx,
                        border_color: t.accent.gamma_multiply(0.55),
                        stroke_cache_scope: "drag_ghost",
                    },
                );
                draw_drag_ghost_caption(ui, src_idx, drop_target, sheet_w, t);
            });
        });
    ctx.data_mut(|d| d.insert_temp(size_id, frame_resp.response.rect.size()));
}

fn page_tile_tooltip(app: &DrawCraftApp, page: &NotebookPage, page_idx: usize) -> String {
    let (preset, orient) = drawcraft_doc::PageResolutionPreset::detect(page.width, page.height);
    let size_summary = format!(
        "{} · {} ({}×{})",
        preset.display_name(),
        orient.label(),
        page.width,
        page.height
    );
    if app.view.pages_select_mode {
        format!("Select or deselect Page {}", page_idx + 1)
    } else if let Some(backdrop) = page.pdf_backdrop {
        format!(
            "Jump to Page {} (PDF page {} · {size_summary}) — drag thumbnail to reorder",
            page_idx + 1,
            backdrop.page_index + 1
        )
    } else {
        format!(
            "Jump to Page {} ({} • {} • {size_summary}) — drag thumbnail to reorder",
            page_idx + 1,
            page.paper_template.label(),
            page.paper_color.label()
        )
    }
}

fn handle_page_tile_click(app: &mut DrawCraftApp, ui: &egui::Ui, page_idx: usize) {
    let shift_held = ui.input(|i| i.modifiers.shift);
    if app.view.pages_select_mode || shift_held {
        if !app.view.pages_select_mode {
            app.view.pages_select_mode = true;
        }
        if let Some(pos) = app
            .view
            .selected_page_indices
            .iter()
            .position(|&i| i == page_idx)
        {
            app.view.selected_page_indices.remove(pos);
        } else {
            app.view.selected_page_indices.push(page_idx);
        }
    } else {
        crate::canvas::commit_pending_text(app);
        let _ = app
            .session
            .execute("page.select", &json!({ "index": page_idx }));
        app.mesh_cache.clear();
        app.view.fit_requested = true;
        ui.ctx().request_repaint();
    }
}

struct TileVisualStyle {
    fill: Color32,
    border_color: Color32,
    stroke_w: f32,
    label_color: Color32,
    is_drag_source: bool,
}

struct TileVisualQuery {
    page_idx: usize,
    selected: bool,
    batch_selected: bool,
    modal_open: bool,
    drag_source: Option<usize>,
    prev_hovered: bool,
}

fn compute_tile_visual_style(ui: &egui::Ui, q: TileVisualQuery, t: &Tokens) -> TileVisualStyle {
    let anim_base_id = ui.make_persistent_id(("drawcraft_page_tile_anim", q.page_idx));
    let select_t = crate::motion::hover_factor(ui.ctx(), anim_base_id.with("sel"), q.selected);
    let hover_t = crate::motion::hover_factor(
        ui.ctx(),
        anim_base_id.with("hov"),
        q.prev_hovered && !q.selected,
    );

    let is_drag_source = !q.modal_open && q.drag_source == Some(q.page_idx);
    let is_drop_target =
        !q.modal_open && q.drag_source.is_some_and(|src| src != q.page_idx) && q.prev_hovered;

    let rest_fill = crate::motion::lerp_color(t.field_bg, t.border.gamma_multiply(0.32), hover_t);
    let base_fill = crate::motion::lerp_color(
        rest_fill,
        t.accent_soft,
        if q.batch_selected { 1.0 } else { select_t },
    );
    let fill = if is_drop_target {
        crate::motion::lerp_color(t.field_bg, t.accent_soft, 0.80)
    } else if is_drag_source {
        t.toolbar_bg
    } else {
        base_fill
    };

    let rest_border =
        crate::motion::lerp_color(t.border, t.text_muted.gamma_multiply(0.6), hover_t);
    let border_color = if is_drop_target || q.batch_selected {
        t.accent
    } else if is_drag_source {
        t.accent.gamma_multiply(0.45)
    } else {
        crate::motion::lerp_color(rest_border, t.accent, select_t)
    };
    let stroke_w = if is_drop_target || q.batch_selected {
        2.0
    } else if is_drag_source {
        1.5
    } else {
        crate::motion::lerp_f32(1.0, 1.5, select_t)
    };
    let label_color = crate::motion::lerp_color(t.text, t.accent, select_t);

    TileVisualStyle {
        fill,
        border_color,
        stroke_w,
        label_color,
        is_drag_source,
    }
}

fn finish_tile_interaction(
    resp: Option<egui::Response>,
    cursor: egui::CursorIcon,
    tooltip: Option<&str>,
) -> (bool, bool, bool, Option<egui::Response>) {
    let Some(r) = resp else {
        return (false, false, false, None);
    };
    let drag_started = r.drag_started_by(egui::PointerButton::Primary);
    let holding = r.is_pointer_button_down_on();
    let r = r.on_hover_cursor(cursor);
    let r = if let Some(tip) = tooltip {
        r.on_hover_text(tip)
    } else {
        r
    };
    let clicked = r.clicked();
    (drag_started, holding, clicked, Some(r))
}

fn attach_page_tile_context_menu(
    app: &mut DrawCraftApp,
    resp: &egui::Response,
    page_idx: usize,
    t: &Tokens,
) {
    egui::Popup::context_menu(resp).width(196.0).show(|ui| {
        let targets = if app.view.pages_select_mode && !app.view.selected_page_indices.is_empty() {
            app.view.selected_page_indices.clone()
        } else {
            vec![page_idx]
        };
        let tip = if targets.len() > 1 {
            format!("Open Export modal for {} selected pages", targets.len())
        } else {
            format!("Open Export modal for Page {}", page_idx + 1)
        };
        if crate::library_folders::menu_item(ui, Icon::Export, "Export", true, false, t)
            .on_hover_text(tip)
            .clicked()
        {
            crate::export_modal::open_for_pages(app, targets);
            ui.close();
        }
    });
}

fn draw_page_tile(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    args: TileRenderArgs<'_>,
    interaction: &mut GridInteractionState,
    t: &Tokens,
) -> TileDrawResult {
    let page_idx = args.page_idx;
    let hover_mem_id = ui.make_persistent_id(("drawcraft_page_tile_hovered", page_idx));
    let prev_hovered = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(hover_mem_id))
        .unwrap_or(false);

    let batch_selected =
        app.view.pages_select_mode && app.view.selected_page_indices.contains(&page_idx);
    let modal_open = app.view.active_modal.is_some();
    let drag_source = app.view.pages_drag_source;
    let style = compute_tile_visual_style(
        ui,
        TileVisualQuery {
            page_idx,
            selected: args.selected,
            batch_selected,
            modal_open,
            drag_source,
            prev_hovered,
        },
        t,
    );

    let can_drag =
        !app.view.pages_select_mode && !app.view.pages_bookmarked_only && args.total_pages > 1;
    let sense = if can_drag {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::click()
    };

    let mut upper_resp: Option<egui::Response> = None;
    let mut footer = TileFooterResult {
        sub_action_taken: false,
        bookmark_rect: egui::Rect::NOTHING,
        label_resp: None,
    };

    // Keep the outer Frame hover-only so it never steals clicks from footer buttons
    // (bookmark, move up/down); attach tile click/drag strictly to the sheet preview
    // and page label.
    let resp = egui::Frame::NONE
        .fill(style.fill)
        .stroke(egui::Stroke::new(style.stroke_w, style.border_color))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(7))
        .show(ui, |ui| {
            if style.is_drag_source {
                ui.multiply_opacity(0.42);
            }
            ui.set_width(ui.available_width());
            let body = ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 0.0),
                egui::Layout::top_down(egui::Align::Center),
                |ui| {
                    paint_page_tile_sheet(
                        app,
                        ui,
                        args.page,
                        page_idx,
                        style.border_color,
                        batch_selected,
                        t,
                    );
                },
            );
            upper_resp = Some(body.response.interact(sense));
            ui.add_space(4.0);
            footer = draw_page_tile_footer(
                app,
                ui,
                args.page,
                page_idx,
                args.total_pages,
                args.selected,
                sense,
                style.label_color,
                &mut interaction.pending_move,
                t,
            );
        })
        .response;

    let tile_rect = resp.rect;
    let pointer_on_tile = ui.rect_contains_pointer(tile_rect);
    if pointer_on_tile {
        interaction.hovered_tile = Some(page_idx);
    }
    if pointer_on_tile != prev_hovered {
        ui.ctx().request_repaint();
    }

    let hover_cursor = if drag_source.is_some() {
        egui::CursorIcon::Grabbing
    } else {
        egui::CursorIcon::PointingHand
    };
    let tip = (drag_source.is_none() && pointer_on_tile)
        .then(|| page_tile_tooltip(app, args.page, page_idx));
    let (upper_drag, upper_hold, upper_click, upper_done) =
        finish_tile_interaction(upper_resp, hover_cursor, tip.as_deref());
    let (label_drag, label_hold, label_click, label_done) =
        finish_tile_interaction(footer.label_resp, hover_cursor, tip.as_deref());

    if let Some(ref r) = upper_done {
        attach_page_tile_context_menu(app, r, page_idx, t);
    }
    if let Some(ref r) = label_done {
        attach_page_tile_context_menu(app, r, page_idx, t);
    }

    if can_drag
        && drag_source.is_none()
        && !modal_open
        && (upper_hold || label_hold)
        && ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
    {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }

    let drag_started =
        !footer.sub_action_taken && can_drag && !modal_open && (upper_drag || label_drag);
    if drag_started {
        app.view.pages_drag_source = Some(page_idx);
        ui.ctx().request_repaint();
    }
    if interaction.pointer_released
        && pointer_on_tile
        && !modal_open
        && let Some(src) = app.view.pages_drag_source
        && src != page_idx
    {
        interaction.pending_move = Some(PendingPageMove {
            from: src,
            to: page_idx,
        });
    }

    ui.ctx()
        .data_mut(|d| d.insert_temp(hover_mem_id, pointer_on_tile));
    if (upper_click || label_click) && !footer.sub_action_taken && drag_source.is_none() {
        handle_page_tile_click(app, ui, page_idx);
    }
    TileDrawResult {
        tile_rect,
        bookmark_rect: footer.bookmark_rect,
    }
}

fn mini_icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: egui::Vec2,
    shrink: f32,
    tooltip: &str,
    t: &Tokens,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(4),
            t.border.gamma_multiply(0.35),
        );
    }
    paint_icon(
        ui.painter(),
        rect.shrink(shrink),
        icon,
        if resp.hovered() {
            t.accent
        } else {
            t.text_muted
        },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

#[allow(clippy::too_many_arguments)]
fn draw_page_tile_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    page: &NotebookPage,
    page_idx: usize,
    total_pages: usize,
    selected: bool,
    tile_sense: egui::Sense,
    label_color: Color32,
    pending_move: &mut Option<PendingPageMove>,
    t: &Tokens,
) -> TileFooterResult {
    let mut sub_action_taken = false;
    let mut bookmark_rect = egui::Rect::NOTHING;
    let mut label_resp = None;
    ui.horizontal(|ui| {
        ui.set_width(ui.available_width());
        let has_move_arrows = selected && !app.view.pages_bookmarked_only && total_pages > 1;
        let right_reserve = if has_move_arrows { 54.0 } else { 20.0 };
        let label_max_w = (ui.available_width() - right_reserve).max(28.0);
        ui.allocate_ui_with_layout(
            egui::vec2(label_max_w, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                label_resp = Some(
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("Page {}", page_idx + 1))
                                .size(11.0)
                                .strong()
                                .color(label_color),
                        )
                        .truncate()
                        .sense(tile_sense),
                    ),
                );
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let (bm_rect, bm_resp) =
                ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            bookmark_rect = bm_rect;
            if page.bookmarked {
                let pill_fill = crate::motion::lerp_color(t.danger_bg, t.danger_border, 0.35);
                ui.painter().rect(
                    bm_rect,
                    egui::CornerRadius::same(4),
                    pill_fill,
                    egui::Stroke::new(1.0, t.danger_border),
                    egui::StrokeKind::Inside,
                );
            } else if bm_resp.hovered() {
                ui.painter().rect_filled(
                    bm_rect,
                    egui::CornerRadius::same(4),
                    t.border.gamma_multiply(0.35),
                );
            }
            let bm_color = if page.bookmarked {
                t.danger_fg
            } else if bm_resp.hovered() {
                t.text
            } else {
                t.text_muted.gamma_multiply(0.75)
            };
            paint_icon(ui.painter(), bm_rect.shrink(2.0), Icon::Bookmark, bm_color);
            if bm_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(if page.bookmarked {
                    "Remove bookmark"
                } else {
                    "Bookmark this page"
                })
                .clicked()
            {
                sub_action_taken = true;
                let _ = app
                    .session
                    .execute("page.setBookmark", &json!({ "index": page_idx }));
            }

            if has_move_arrows {
                if page_idx + 1 < total_pages
                    && mini_icon_button(
                        ui,
                        Icon::MoveDown,
                        egui::vec2(15.0, 18.0),
                        1.5,
                        "Move page later",
                        t,
                    )
                    .clicked()
                {
                    sub_action_taken = true;
                    *pending_move = Some(PendingPageMove {
                        from: page_idx,
                        to: page_idx + 1,
                    });
                }
                if page_idx > 0
                    && mini_icon_button(
                        ui,
                        Icon::MoveUp,
                        egui::vec2(15.0, 18.0),
                        1.5,
                        "Move page earlier",
                        t,
                    )
                    .clicked()
                {
                    sub_action_taken = true;
                    *pending_move = Some(PendingPageMove {
                        from: page_idx,
                        to: page_idx - 1,
                    });
                }
            }
        });
    });
    TileFooterResult {
        sub_action_taken,
        bookmark_rect,
        label_resp,
    }
}

fn draw_batch_selection_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    page_count: usize,
    visible_indices: &[usize],
    t: &Tokens,
) -> FooterLayout {
    let selected = app.view.selected_page_indices.clone();
    let count = selected.len();
    let has_sel = count > 0;
    let can_delete = has_sel && count < page_count;
    let all_visible_selected =
        !visible_indices.is_empty() && visible_indices.iter().all(|idx| selected.contains(idx));

    let footer_rect = card_frame(t)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;

            ui.horizontal(|ui| {
                ui.set_height(28.0);
                ui.label(
                    egui::RichText::new(format!("{count} selected"))
                        .size(13.0)
                        .strong()
                        .color(t.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if compact_icon_button(ui, Icon::Close, Some("Clear"), false, has_sel, t)
                        .on_hover_text("Clear selected pages")
                        .clicked()
                    {
                        app.view.selected_page_indices.clear();
                    }
                    if compact_icon_button(
                        ui,
                        Icon::Check,
                        Some("All"),
                        false,
                        !visible_indices.is_empty() && !all_visible_selected,
                        t,
                    )
                    .on_hover_text("Select all visible pages")
                    .clicked()
                    {
                        app.view.selected_page_indices = visible_indices.to_vec();
                    }
                });
            });

            ui.add_space(3.0);
            let helper_text = match count {
                0 => "Click pages above to select them.".to_string(),
                1 => format!("1 of {page_count} pages ready for batch action."),
                n => format!("{n} of {page_count} pages ready for batch action."),
            };
            ui.horizontal(|ui| {
                ui.set_height(15.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(helper_text)
                            .size(11.0)
                            .color(t.text_muted),
                    )
                    .truncate(),
                );
            });

            ui.add_space(5.0);
            draw_card_hairline(ui, t);
            ui.add_space(5.0);

            let btn_h = 26.0;
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.columns(2, |cols| {
                if sidebar_action_button(
                    &mut cols[0],
                    Icon::Duplicate,
                    "Duplicate",
                    ActionTone::Neutral,
                    has_sel,
                    btn_h,
                    t,
                )
                .on_hover_text("Duplicate all selected pages")
                .clicked()
                {
                    crate::canvas::commit_pending_text(app);
                    let _ = app
                        .session
                        .execute("page.batchDuplicate", &json!({ "indices": selected }));
                    app.view.selected_page_indices.clear();
                    app.mesh_cache.clear();
                }

                if sidebar_action_button(
                    &mut cols[1],
                    Icon::RotateCw,
                    "Rotate 90°",
                    ActionTone::Neutral,
                    has_sel,
                    btn_h,
                    t,
                )
                .on_hover_text("Rotate selected pages 90° clockwise")
                .clicked()
                {
                    crate::canvas::commit_pending_text(app);
                    let _ = app.session.execute(
                        "page.rotate",
                        &json!({ "pages": selected, "quarter_turns": 1 }),
                    );
                    app.mesh_cache.clear();
                    app.view.fit_requested = true;
                }
            });

            ui.add_space(4.0);
            ui.columns(2, |cols| {
                if sidebar_action_button(
                    &mut cols[0],
                    Icon::Transfer,
                    "Transfer",
                    ActionTone::Neutral,
                    has_sel,
                    btn_h,
                    t,
                )
                .on_hover_text("Move or copy selected pages to another or new notebook")
                .clicked()
                {
                    app.view.transfer_pages_list = selected.clone();
                    app.view.transfer_merge_whole_notebook = None;
                    modal::open(app, ActiveModal::TransferPages);
                }

                if sidebar_action_button(
                    &mut cols[1],
                    Icon::Export,
                    "Export",
                    ActionTone::Neutral,
                    has_sel,
                    btn_h,
                    t,
                )
                .on_hover_text("Open Export modal for the selected pages")
                .clicked()
                {
                    app.view.export_format = crate::FileDialogKind::Pdf;
                    crate::export_modal::open_for_pages(app, selected.clone());
                }
            });

            ui.add_space(4.0);
            let remove_label = match count {
                0 => "Remove Selected".to_string(),
                1 => "Remove 1 Page".to_string(),
                n => format!("Remove {n} Pages"),
            };
            let remove_tip = if !has_sel {
                "Select one or more pages above to remove them".to_string()
            } else if !can_delete {
                "At least one page must remain in the notebook".to_string()
            } else {
                format!("Delete {count} selected page(s) from this notebook")
            };
            if sidebar_action_button(
                ui,
                Icon::Trash,
                &remove_label,
                ActionTone::Danger,
                can_delete,
                btn_h,
                t,
            )
            .on_hover_text(remove_tip)
            .clicked()
            {
                crate::canvas::commit_pending_text(app);
                let _ = app
                    .session
                    .execute("page.batchRemove", &json!({ "indices": selected }));
                app.view.selected_page_indices.clear();
                app.mesh_cache.clear();
                app.view.fit_requested = true;
            }
        })
        .response
        .rect;

    FooterLayout {
        footer_rect,
        prev_rect: egui::Rect::NOTHING,
        next_rect: egui::Rect::NOTHING,
        toggle_rect: egui::Rect::NOTHING,
        measured_drawer_h: None,
        cur_drawer_h: 0.0,
    }
}

#[derive(Clone)]
struct CachedThumbStroke {
    color: Color32,
    width: f32,
    is_highlighter: bool,
    pts: Vec<egui::Pos2>,
}

#[derive(Clone, PartialEq, Eq)]
struct ThumbCacheKey {
    notebook_idx: usize,
    page_idx: usize,
    width_bits: u32,
    height_bits: u32,
    page_width: u32,
    page_height: u32,
    stroke_count: usize,
    stroke_digest: u64,
    text_count: usize,
    shape_count: usize,
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
    paper_color: drawcraft_doc::PaperColor,
}

#[derive(Clone)]
struct ThumbForegroundCache {
    key: ThumbCacheKey,
    stroke_sources: Vec<Arc<InkStroke>>,
    text_sources: Vec<Arc<drawcraft_doc::TextItem>>,
    shape_sources: Vec<Arc<drawcraft_doc::GeometricShape>>,
    stroke_image: Option<egui::ColorImage>,
    stroke_texture: Option<egui::TextureHandle>,
    cached_origin: egui::Pos2,
    shapes: std::sync::Arc<Vec<egui::Shape>>,
}

fn arcs_match<T>(cached: &[Arc<T>], current: &[Arc<T>]) -> bool {
    cached.len() == current.len() && cached.iter().zip(current).all(|(a, b)| Arc::ptr_eq(a, b))
}

fn decimate_thumb_points(
    samples: &[drawcraft_doc::StrokeSample],
    sx: f32,
    sy: f32,
) -> Vec<egui::Pos2> {
    let mut pts = Vec::with_capacity(samples.len().min(64));
    let mut last_kept: Option<egui::Pos2> = None;
    let last_idx = samples.len().saturating_sub(1);
    for (idx, sample) in samples.iter().enumerate() {
        let pt = sample.point();
        let x = (pt.x as f32) * sx;
        let y = (pt.y as f32) * sy;
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        let pos = egui::pos2(x, y);
        let keep = match last_kept {
            None => true,
            Some(_) if idx == last_idx => true,
            Some(prev) => prev.distance_sq(pos) >= 1.5625,
        };
        if keep {
            pts.push(pos);
            last_kept = Some(pos);
        }
    }
    pts
}

fn build_single_thumb_stroke(
    sx: f32,
    sy: f32,
    eff_paper: drawcraft_doc::PaperColor,
    theme: drawcraft_doc::UiTheme,
    eff_contrast: bool,
    stroke: &InkStroke,
) -> Option<CachedThumbStroke> {
    let pts = decimate_thumb_points(&stroke.smoothed_points, sx, sy);
    if pts.is_empty() {
        return None;
    }
    let is_highlighter = stroke.pen.kind == PenKind::Highlighter;
    let resolved_color =
        eff_paper.resolve_ink_color(theme, eff_contrast, stroke.pen.color, is_highlighter);
    let mut color = to_color32(resolved_color);
    if is_highlighter {
        let alpha = ((f32::from(color.a()) * 0.42).round() as u8).max(40);
        color = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
    }
    let width = (stroke.pen.size * sx).clamp(0.8, 3.2);
    Some(CachedThumbStroke {
        color,
        width,
        is_highlighter,
        pts,
    })
}

fn build_thumbnail_strokes(
    sheet_size: egui::Vec2,
    page: &NotebookPage,
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
) -> Vec<CachedThumbStroke> {
    if page.strokes.is_empty() {
        return Vec::new();
    }
    let sx = sheet_size.x / (page.width.max(1) as f32);
    let sy = sheet_size.y / (page.height.max(1) as f32);
    let (eff_contrast, eff_paper) = if page.pdf_backdrop.is_some() {
        (false, drawcraft_doc::PaperColor::PureWhite)
    } else {
        (auto_contrast, page.paper_color)
    };
    let mut out = Vec::with_capacity(page.strokes.len());
    for stroke in &page.strokes {
        if let Some(built) =
            build_single_thumb_stroke(sx, sy, eff_paper, theme, eff_contrast, stroke)
        {
            out.push(built);
        }
    }
    out
}

#[inline]
fn blend_premul_src_over(dst: Color32, src: Color32, cov: f32) -> Color32 {
    if cov <= 0.002 {
        return dst;
    }
    let src_a = f32::from(src.a()) * cov;
    let inv_sa = (255.0 - src_a) / 255.0;
    let r = (f32::from(src.r()) * cov + f32::from(dst.r()) * inv_sa)
        .round()
        .clamp(0.0, 255.0) as u8;
    let g = (f32::from(src.g()) * cov + f32::from(dst.g()) * inv_sa)
        .round()
        .clamp(0.0, 255.0) as u8;
    let b = (f32::from(src.b()) * cov + f32::from(dst.b()) * inv_sa)
        .round()
        .clamp(0.0, 255.0) as u8;
    let a = (src_a + f32::from(dst.a()) * inv_sa)
        .round()
        .clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(r, g, b, a)
}

#[inline]
fn segment_dist_sq(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq > 1e-6 {
        (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let qx = ax + t * dx - px;
    let qy = ay + t * dy - py;
    qx * qx + qy * qy
}

#[inline]
fn clipped_pixel_span(
    lo: f32,
    hi: f32,
    pad: f32,
    min_bound: isize,
    max_bound: isize,
) -> Option<(usize, usize)> {
    if !(lo.is_finite() && hi.is_finite() && pad.is_finite())
        || min_bound < 0
        || max_bound < min_bound
    {
        return None;
    }
    let (min_v, max_v) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let safe_pad = pad.max(0.0);
    let start = ((min_v - safe_pad).floor() as isize).max(min_bound);
    let end = ((max_v + safe_pad).ceil() as isize).min(max_bound);
    if end < start {
        return None;
    }
    Some((start as usize, end as usize))
}

fn rasterize_capsule_direct(
    pixels: &mut [Color32],
    tex_size: [usize; 2],
    a: egui::Pos2,
    b: egui::Pos2,
    radius: f32,
    color: Color32,
) {
    let [tex_w, tex_h] = tex_size;
    if tex_w == 0
        || tex_h == 0
        || !radius.is_finite()
        || radius < 0.0
        || !a.x.is_finite()
        || !a.y.is_finite()
        || !b.x.is_finite()
        || !b.y.is_finite()
    {
        return;
    }
    let pad = radius + 0.85;
    let Some((min_x, max_x)) = clipped_pixel_span(a.x, b.x, pad, 0, tex_w as isize - 1) else {
        return;
    };
    let Some((min_y, max_y)) = clipped_pixel_span(a.y, b.y, pad, 0, tex_h as isize - 1) else {
        return;
    };
    let outer_sq = pad * pad;
    for py in min_y..=max_y {
        let cy = py as f32 + 0.5;
        let row = py * tex_w;
        for px in min_x..=max_x {
            let cx = px as f32 + 0.5;
            let d_sq = segment_dist_sq(cx, cy, a.x, a.y, b.x, b.y);
            if d_sq >= outer_sq {
                continue;
            }
            let cov = (radius + 0.65 - d_sq.sqrt()).clamp(0.0, 1.0);
            if let Some(dst) = pixels.get_mut(row + px) {
                *dst = blend_premul_src_over(*dst, color, cov);
            }
        }
    }
}

fn rasterize_stroke_into_mask(
    pixels: &mut [Color32],
    tex_size: [usize; 2],
    scale: [f32; 2],
    stroke: &CachedThumbStroke,
    scratch: &mut Vec<f32>,
) {
    let [tex_w, tex_h] = tex_size;
    let [rx, ry] = scale;
    if tex_w == 0
        || tex_h == 0
        || !rx.is_finite()
        || !ry.is_finite()
        || rx <= 0.0
        || ry <= 0.0
        || !stroke.width.is_finite()
    {
        return;
    }
    let radius = (stroke.width * 0.5 * rx).max(0.75);
    let pad = radius + 0.85;
    let (mut min_xf, mut min_yf) = (f32::INFINITY, f32::INFINITY);
    let (mut max_xf, mut max_yf) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for pt in &stroke.pts {
        let x = pt.x * rx;
        let y = pt.y * ry;
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        min_xf = min_xf.min(x);
        min_yf = min_yf.min(y);
        max_xf = max_xf.max(x);
        max_yf = max_yf.max(y);
    }
    let Some((min_x, max_x)) = clipped_pixel_span(min_xf, max_xf, pad, 0, tex_w as isize - 1)
    else {
        return;
    };
    let Some((min_y, max_y)) = clipped_pixel_span(min_yf, max_yf, pad, 0, tex_h as isize - 1)
    else {
        return;
    };
    let bw = max_x - min_x + 1;
    let bh = max_y - min_y + 1;
    let total = bw.saturating_mul(bh);
    scratch.clear();
    scratch.resize(total, 0.0);

    let outer_sq = pad * pad;
    let seg_count = stroke.pts.len().saturating_sub(1).max(1);
    for idx in 0..seg_count {
        let p0 = stroke.pts[idx];
        let p1 = stroke.pts.get(idx + 1).copied().unwrap_or(p0);
        let ax = p0.x * rx;
        let ay = p0.y * ry;
        let bx = p1.x * rx;
        let by = p1.y * ry;
        if !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite()) {
            continue;
        }
        let Some((sx0, sx1)) = clipped_pixel_span(ax, bx, pad, min_x as isize, max_x as isize)
        else {
            continue;
        };
        let Some((sy0, sy1)) = clipped_pixel_span(ay, by, pad, min_y as isize, max_y as isize)
        else {
            continue;
        };
        for py in sy0..=sy1 {
            let cy = py as f32 + 0.5;
            let row = (py - min_y) * bw;
            for px in sx0..=sx1 {
                let cx = px as f32 + 0.5;
                let d_sq = segment_dist_sq(cx, cy, ax, ay, bx, by);
                if d_sq < outer_sq
                    && let Some(cell) = scratch.get_mut(row + (px - min_x))
                {
                    let cov = (radius + 0.65 - d_sq.sqrt()).clamp(0.0, 1.0);
                    *cell = cell.max(cov);
                }
            }
        }
    }

    for ly in 0..bh {
        let py = min_y + ly;
        let dst_row = py * tex_w;
        let src_row = ly * bw;
        for lx in 0..bw {
            let cov = scratch[src_row + lx];
            if cov > 0.002 {
                let px = min_x + lx;
                if let Some(dst) = pixels.get_mut(dst_row + px) {
                    *dst = blend_premul_src_over(*dst, stroke.color, cov);
                }
            }
        }
    }
}

fn rasterize_single_thumb_stroke(
    image: &mut egui::ColorImage,
    sheet_size: egui::Vec2,
    stroke: &CachedThumbStroke,
    scratch: &mut Vec<f32>,
) {
    if stroke.pts.is_empty() {
        return;
    }
    let tex_size = image.size;
    let sx = (tex_size[0] as f32) / sheet_size.x.max(1.0);
    let sy = (tex_size[1] as f32) / sheet_size.y.max(1.0);
    if stroke.color.a() < 255 {
        rasterize_stroke_into_mask(&mut image.pixels, tex_size, [sx, sy], stroke, scratch);
        return;
    }
    let radius = (stroke.width * 0.5 * sx).max(0.75);
    let seg_count = stroke.pts.len().saturating_sub(1).max(1);
    for idx in 0..seg_count {
        let p0 = stroke.pts[idx];
        let p1 = stroke.pts.get(idx + 1).copied().unwrap_or(p0);
        let a = egui::pos2(p0.x * sx, p0.y * sy);
        let b = egui::pos2(p1.x * sx, p1.y * sy);
        rasterize_capsule_direct(&mut image.pixels, tex_size, a, b, radius, stroke.color);
    }
}

fn rasterize_thumbnail_strokes_image(
    sheet_size: egui::Vec2,
    strokes: &[CachedThumbStroke],
) -> egui::ColorImage {
    let tex_w = ((sheet_size.x * 2.0).round() as usize).clamp(16, 512);
    let tex_h = ((sheet_size.y * 2.0).round() as usize).clamp(16, 512);
    let mut image =
        egui::ColorImage::new([tex_w, tex_h], vec![Color32::TRANSPARENT; tex_w * tex_h]);
    let mut scratch = Vec::new();
    for pass_hl in [true, false] {
        for stroke in strokes {
            if stroke.is_highlighter == pass_hl {
                rasterize_single_thumb_stroke(&mut image, sheet_size, stroke, &mut scratch);
            }
        }
    }
    image
}

fn paint_thumb_texture_quad(p: &egui::Painter, rect: egui::Rect, tex_id: egui::TextureId) {
    let mut mesh = egui::Mesh::with_texture(tex_id);
    mesh.add_rect_with_uv(
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    p.add(egui::Shape::mesh(mesh));
}

struct ThumbForegroundTarget<'a> {
    page: &'a NotebookPage,
    notebook_idx: usize,
    page_idx: usize,
    scope: &'static str,
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
    defer_rebuild: bool,
}

fn try_append_thumb_stroke(
    entry: &mut ThumbForegroundCache,
    key: &ThumbCacheKey,
    sheet_rect: egui::Rect,
    page: &NotebookPage,
) -> bool {
    let prev_n = entry.stroke_sources.len();
    if page.strokes.len() != prev_n + 1
        || !page.shapes.is_empty()
        || !page.texts.is_empty()
        || entry.key.notebook_idx != key.notebook_idx
        || entry.key.page_idx != key.page_idx
        || entry.key.width_bits != key.width_bits
        || entry.key.height_bits != key.height_bits
        || entry.key.page_width != key.page_width
        || entry.key.page_height != key.page_height
        || entry.key.theme != key.theme
        || entry.key.auto_contrast != key.auto_contrast
        || entry.key.paper_color != key.paper_color
        || !arcs_match(&entry.stroke_sources, &page.strokes[..prev_n])
    {
        return false;
    }
    let Some(last) = page.strokes.last() else {
        return false;
    };
    if last.pen.kind == PenKind::Highlighter {
        return false;
    }
    let (Some(img), Some(tex)) = (entry.stroke_image.as_mut(), entry.stroke_texture.as_mut())
    else {
        return false;
    };
    let sx = sheet_rect.width() / (page.width.max(1) as f32);
    let sy = sheet_rect.height() / (page.height.max(1) as f32);
    if let Some(built) =
        build_single_thumb_stroke(sx, sy, key.paper_color, key.theme, key.auto_contrast, last)
    {
        let mut scratch = Vec::new();
        rasterize_single_thumb_stroke(img, sheet_rect.size(), &built, &mut scratch);
        tex.set(img.clone(), egui::TextureOptions::LINEAR);
    }
    entry.key = key.clone();
    entry.stroke_sources.push(Arc::clone(last));
    true
}

fn build_thumbnail_foreground_assets(
    p: &egui::Painter,
    sheet_rect: egui::Rect,
    page: &NotebookPage,
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
) -> (Option<egui::ColorImage>, Vec<egui::Shape>) {
    let sheet_size = sheet_rect.size();
    let tex_w = ((sheet_size.x * 2.0).round() as usize).clamp(16, 512);
    let tex_h = ((sheet_size.y * 2.0).round() as usize).clamp(16, 512);
    let mut image =
        egui::ColorImage::new([tex_w, tex_h], vec![Color32::TRANSPARENT; tex_w * tex_h]);

    if !page.strokes.is_empty() {
        let strokes = build_thumbnail_strokes(sheet_size, page, theme, auto_contrast);
        let mut scratch = Vec::new();
        for pass_hl in [true, false] {
            for stroke in &strokes {
                if stroke.is_highlighter == pass_hl {
                    rasterize_single_thumb_stroke(&mut image, sheet_size, stroke, &mut scratch);
                }
            }
        }
    }

    let mut heading_shapes = Vec::new();
    append_thumbnail_text_assets(
        &mut image,
        &mut heading_shapes,
        p,
        sheet_rect,
        page,
        theme,
        auto_contrast,
    );
    rasterize_thumbnail_shapes(&mut image, sheet_size, page, theme, auto_contrast);

    let has_pixels = !page.strokes.is_empty() || !page.shapes.is_empty() || !page.texts.is_empty();
    (has_pixels.then_some(image), heading_shapes)
}

fn paint_thumbnail_foreground_cached(
    ui: &egui::Ui,
    sheet_rect: egui::Rect,
    target: ThumbForegroundTarget<'_>,
) {
    let page = target.page;
    if page.strokes.is_empty() && page.texts.is_empty() && page.shapes.is_empty() {
        return;
    }
    let stroke_digest = page
        .strokes
        .iter()
        .fold(0u64, |acc, s| acc.rotate_left(7) ^ s.id.0);
    let key = ThumbCacheKey {
        notebook_idx: target.notebook_idx,
        page_idx: target.page_idx,
        width_bits: (sheet_rect.width() * 2.0).round().to_bits(),
        height_bits: (sheet_rect.height() * 2.0).round().to_bits(),
        page_width: page.width,
        page_height: page.height,
        stroke_count: page.strokes.len(),
        stroke_digest,
        text_count: page.texts.len(),
        shape_count: page.shapes.len(),
        theme: target.theme,
        auto_contrast: target.auto_contrast && page.pdf_backdrop.is_none(),
        paper_color: if page.pdf_backdrop.is_some() {
            drawcraft_doc::PaperColor::PureWhite
        } else {
            page.paper_color
        },
    };
    let cache_id =
        ui.make_persistent_id(("drawcraft_thumb_fg_cache", target.scope, target.page_idx));
    let mut entry = ui
        .ctx()
        .data(|d| d.get_temp::<ThumbForegroundCache>(cache_id));

    let exact_hit = entry.as_ref().is_some_and(|e| {
        e.key == key
            && arcs_match(&e.stroke_sources, &page.strokes)
            && arcs_match(&e.text_sources, &page.texts)
            && arcs_match(&e.shape_sources, &page.shapes)
    });

    if !exact_hit && !(target.defer_rebuild && entry.is_some()) {
        let appended = entry
            .as_mut()
            .is_some_and(|e| try_append_thumb_stroke(e, &key, sheet_rect, page));
        if !appended {
            let (maybe_img, built_shapes) = build_thumbnail_foreground_assets(
                ui.painter(),
                sheet_rect,
                page,
                target.theme,
                target.auto_contrast,
            );
            let (stroke_image, stroke_texture) = if let Some(img) = maybe_img {
                let tex = ui.ctx().load_texture(
                    format!(
                        "thumb_fg_{}_{}_{}",
                        target.scope, target.notebook_idx, target.page_idx
                    ),
                    img.clone(),
                    egui::TextureOptions::LINEAR,
                );
                (Some(img), Some(tex))
            } else {
                (None, None)
            };
            entry = Some(ThumbForegroundCache {
                key,
                stroke_sources: page.strokes.clone(),
                text_sources: page.texts.clone(),
                shape_sources: page.shapes.clone(),
                stroke_image,
                stroke_texture,
                cached_origin: sheet_rect.min,
                shapes: std::sync::Arc::new(built_shapes),
            });
        }
        if let Some(updated) = &entry {
            ui.ctx()
                .data_mut(|d| d.insert_temp(cache_id, updated.clone()));
        }
    }

    let Some(entry) = entry else {
        return;
    };
    let clipped = ui.painter().with_clip_rect(sheet_rect);
    if let Some(tex) = &entry.stroke_texture {
        paint_thumb_texture_quad(&clipped, sheet_rect, tex.id());
    }
    if !entry.shapes.is_empty() {
        let delta = sheet_rect.min - entry.cached_origin;
        let needs_translate = delta.length_sq() > 1e-6;
        for shape in entry.shapes.iter() {
            let mut s = shape.clone();
            if needs_translate {
                s.translate(delta);
            }
            clipped.add(s);
        }
    }
}

fn paint_thumbnail_images(ui: &egui::Ui, sheet_rect: egui::Rect, page: &NotebookPage) {
    if page.images.is_empty() {
        return;
    }
    let sx = sheet_rect.width() / page.width.max(1) as f32;
    let sy = sheet_rect.height() / page.height.max(1) as f32;
    let clipped = ui.painter().with_clip_rect(sheet_rect);
    for image in &page.images {
        let clean = image.sanitized();
        let Some(handle) =
            crate::canvas::cached_page_image_texture(ui.ctx(), "drawcraft_thumb_img", image)
        else {
            continue;
        };
        let corners_page = clean.corners();
        let to_thumb = |pt: drawcraft_geom::Point| {
            egui::pos2(
                sheet_rect.left() + pt.x as f32 * sx,
                sheet_rect.top() + pt.y as f32 * sy,
            )
        };
        let corners = [
            to_thumb(corners_page[0]),
            to_thumb(corners_page[1]),
            to_thumb(corners_page[2]),
            to_thumb(corners_page[3]),
        ];
        crate::canvas::paint_textured_image_quad(&clipped, handle.id(), corners);
    }
}

#[derive(Clone, PartialEq, Eq)]
struct HomeThumbStrokeKey {
    created_at_ms: i64,
    width_bits: u32,
    height_bits: u32,
    page_width: u32,
    page_height: u32,
    stroke_count: usize,
    stroke_digest: u64,
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
    paper_color: drawcraft_doc::PaperColor,
}

#[derive(Clone)]
struct HomeThumbStrokeCache {
    key: HomeThumbStrokeKey,
    stroke_sources: Vec<Arc<InkStroke>>,
    texture: egui::TextureHandle,
}

pub(crate) fn paint_thumbnail_strokes_cached(
    ui: &egui::Ui,
    sheet_rect: egui::Rect,
    page: &NotebookPage,
    doc_id: (i64, &str),
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
) {
    if page.strokes.is_empty() {
        return;
    }
    let stroke_digest = page
        .strokes
        .iter()
        .fold(0u64, |acc, s| acc.rotate_left(7) ^ s.id.0);
    let key = HomeThumbStrokeKey {
        created_at_ms: doc_id.0,
        width_bits: sheet_rect.width().to_bits(),
        height_bits: sheet_rect.height().to_bits(),
        page_width: page.width,
        page_height: page.height,
        stroke_count: page.strokes.len(),
        stroke_digest,
        theme,
        auto_contrast: auto_contrast && page.pdf_backdrop.is_none(),
        paper_color: if page.pdf_backdrop.is_some() {
            drawcraft_doc::PaperColor::PureWhite
        } else {
            page.paper_color
        },
    };
    let cache_id = ui.make_persistent_id(("drawcraft_home_thumb_strokes", doc_id.0, doc_id.1));
    let cached = ui
        .ctx()
        .data(|d| d.get_temp::<HomeThumbStrokeCache>(cache_id));
    let texture = match cached {
        Some(entry) if entry.key == key && arcs_match(&entry.stroke_sources, &page.strokes) => {
            entry.texture
        }
        _ => {
            let strokes = build_thumbnail_strokes(sheet_rect.size(), page, theme, auto_contrast);
            let img = rasterize_thumbnail_strokes_image(sheet_rect.size(), &strokes);
            let tex = ui.ctx().load_texture(
                format!("home_thumb_{}_{}", doc_id.0, doc_id.1),
                img,
                egui::TextureOptions::LINEAR,
            );
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    cache_id,
                    HomeThumbStrokeCache {
                        key,
                        stroke_sources: page.strokes.clone(),
                        texture: tex.clone(),
                    },
                );
            });
            tex
        }
    };
    let clipped = ui.painter().with_clip_rect(sheet_rect);
    paint_thumb_texture_quad(&clipped, sheet_rect, texture.id());
}

fn rasterize_thumb_micro_line(
    pixels: &mut [Color32],
    tex_size: [usize; 2],
    origin: [f32; 2],
    line_text: &str,
    char_adv_px: f32,
    bar_h_px: f32,
    color: Color32,
) {
    let [tex_w, tex_h] = tex_size;
    if tex_w == 0
        || tex_h == 0
        || !origin[0].is_finite()
        || !origin[1].is_finite()
        || !char_adv_px.is_finite()
        || char_adv_px <= 0.0
        || !bar_h_px.is_finite()
    {
        return;
    }
    let Some((y0, y1)) = clipped_pixel_span(
        origin[1],
        origin[1] + bar_h_px.max(0.0),
        0.0,
        0,
        tex_h as isize - 1,
    ) else {
        return;
    };
    let mut cursor_x = origin[0];
    let space_w = (char_adv_px * 0.7).max(0.8);
    let soft_c = Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        ((f32::from(color.a()) * 0.72).round() as u8).max(40),
    );
    for word in line_text.split_whitespace() {
        let w = (word.chars().count() as f32) * char_adv_px;
        if let Some((x0, x1)) =
            clipped_pixel_span(cursor_x, cursor_x + w, 0.0, 0, tex_w as isize - 1)
        {
            for py in y0..=y1 {
                let row = py * tex_w;
                for px in x0..=x1 {
                    if let Some(dst) = pixels.get_mut(row + px) {
                        *dst = blend_premul_src_over(*dst, soft_c, 1.0);
                    }
                }
            }
        }
        cursor_x += w + space_w;
        if cursor_x >= tex_w as f32 {
            break;
        }
    }
}

fn append_thumbnail_text_assets(
    image: &mut egui::ColorImage,
    out: &mut Vec<egui::Shape>,
    p: &egui::Painter,
    sheet_rect: egui::Rect,
    page: &NotebookPage,
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
) {
    if page.texts.is_empty() {
        return;
    }
    let [tex_w, tex_h] = image.size;
    let sx = sheet_rect.width() / page.width.max(1) as f32;
    let sy = sheet_rect.height() / page.height.max(1) as f32;
    let scale = sx.min(sy);
    let sx_img = (tex_w as f32) / page.width.max(1) as f32;
    let sy_img = (tex_h as f32) / page.height.max(1) as f32;
    let (eff_contrast, eff_paper) = if page.pdf_backdrop.is_some() {
        (false, drawcraft_doc::PaperColor::PureWhite)
    } else {
        (auto_contrast, page.paper_color)
    };
    let mut shaped_lines_emitted = 0_usize;
    for item in &page.texts {
        let item = item.as_ref().sanitized();
        let color = to_color32(eff_paper.resolve_ink_color(theme, eff_contrast, item.color, false));
        let raw_font_size = (item.size * scale).clamp(1.0, 7.0);
        let max_w = item.effective_max_width(page.width);
        let lines = drawcraft_doc::wrap_text_lines(&item.text, max_w, |ch| {
            let base = if item.font == drawcraft_doc::TextFont::Monospace {
                item.size * 0.62
            } else if ch == ' ' {
                item.size * 0.28
            } else {
                item.size * 0.54
            };
            (base + item.letter_spacing).max(1.0)
        });
        if raw_font_size < 2.9 || shaped_lines_emitted >= 24 {
            let char_adv = ((item.size * 0.52) * sx_img).clamp(0.9, 6.0);
            let line_h_img = (item.size * sy_img * item.line_spacing).max(2.0);
            let bar_h = (item.size * sy_img * 0.68).clamp(1.2, 4.5);
            let base_x = (item.x as f32) * sx_img;
            let base_y = (item.y as f32) * sy_img;
            for (line_idx, line) in lines.iter().enumerate() {
                let ly = base_y + line_h_img * (line_idx as f32);
                if ly > tex_h as f32 {
                    break;
                }
                rasterize_thumb_micro_line(
                    &mut image.pixels,
                    [tex_w, tex_h],
                    [base_x, ly],
                    line,
                    char_adv,
                    bar_h,
                    color,
                );
            }
            continue;
        }
        let font_size = raw_font_size.round().clamp(3.0, 7.0);
        let pos = egui::pos2(
            sheet_rect.left() + item.x as f32 * sx,
            sheet_rect.top() + item.y as f32 * sy,
        );
        let family = match item.font {
            drawcraft_doc::TextFont::Sans => egui::FontFamily::Proportional,
            drawcraft_doc::TextFont::Monospace => egui::FontFamily::Monospace,
        };
        let font_id = egui::FontId::new(font_size, family);
        for (line_idx, line) in lines.into_iter().enumerate() {
            if shaped_lines_emitted >= 24 {
                break;
            }
            let line_pos = pos + egui::vec2(0.0, font_size * item.line_spacing * line_idx as f32);
            let galley = p.layout_no_wrap(line, font_id.clone(), color);
            out.push(egui::Shape::Text(egui::epaint::TextShape::new(
                line_pos, galley, color,
            )));
            shaped_lines_emitted += 1;
        }
    }
}

fn decimate_thumb_img_points(
    points: &[drawcraft_geom::Point],
    sx: f32,
    sy: f32,
) -> Vec<egui::Pos2> {
    let mut out = Vec::with_capacity(points.len().min(64));
    let mut last: Option<egui::Pos2> = None;
    let last_idx = points.len().saturating_sub(1);
    for (idx, pt) in points.iter().enumerate() {
        let x = (pt.x as f32) * sx;
        let y = (pt.y as f32) * sy;
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        let pos = egui::pos2(x, y);
        let keep = match last {
            None => true,
            Some(_) if idx == last_idx => true,
            Some(prev) => prev.distance_sq(pos) >= 1.5625,
        };
        if keep {
            out.push(pos);
            last = Some(pos);
        }
    }
    out
}

fn rasterize_polygon_fill(
    pixels: &mut [Color32],
    tex_size: [usize; 2],
    pts: &[egui::Pos2],
    fill: Color32,
) {
    let [tex_w, tex_h] = tex_size;
    if tex_w == 0 || tex_h == 0 || pts.len() < 3 || fill.a() == 0 {
        return;
    }
    let (mut min_yf, mut max_yf) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in pts {
        if !p.x.is_finite() || !p.y.is_finite() {
            return;
        }
        min_yf = min_yf.min(p.y);
        max_yf = max_yf.max(p.y);
    }
    let Some((y0, y1)) = clipped_pixel_span(min_yf, max_yf, 0.0, 0, tex_h as isize - 1) else {
        return;
    };
    let mut xs: Vec<f32> = Vec::with_capacity(16);
    for py in y0..=y1 {
        let cy = py as f32 + 0.5;
        xs.clear();
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            if (a.y <= cy && b.y > cy) || (b.y <= cy && a.y > cy) {
                let t = (cy - a.y) / (b.y - a.y);
                if t.is_finite() {
                    let x = a.x + t * (b.x - a.x);
                    if x.is_finite() {
                        xs.push(x);
                    }
                }
            }
        }
        xs.sort_unstable_by(f32::total_cmp);
        let row = py * tex_w;
        for &[x0, x1] in xs.as_chunks::<2>().0 {
            let left = x0.min(x1);
            let right = x0.max(x1);
            let Some((px0, px1)) = clipped_pixel_span(left, right, 0.0, 0, tex_w as isize - 1)
            else {
                continue;
            };
            for px in px0..=px1 {
                let cov_l = ((px as f32 + 1.0) - left).clamp(0.0, 1.0);
                let cov_r = (right - (px as f32)).clamp(0.0, 1.0);
                let cov = cov_l.min(cov_r);
                if cov > 0.002
                    && let Some(dst) = pixels.get_mut(row + px)
                {
                    *dst = blend_premul_src_over(*dst, fill, cov);
                }
            }
        }
    }
}

fn rasterize_thumb_polyline(
    pixels: &mut [Color32],
    tex_size: [usize; 2],
    pts: &[egui::Pos2],
    closed: bool,
    radius: f32,
    color: Color32,
    line_style: drawcraft_doc::ShapeLineStyle,
) {
    if pts.len() < 2 {
        return;
    }
    let seg_pairs = if closed { pts.len() } else { pts.len() - 1 };
    let Some((dash_raw, gap_raw)) = line_style.dash_pattern((radius * 2.0).max(1.0)) else {
        for i in 0..seg_pairs {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            rasterize_capsule_direct(pixels, tex_size, a, b, radius, color);
        }
        return;
    };
    let dash = f64::from(dash_raw.max(2.0));
    let gap = f64::from(gap_raw.max(2.5));
    let cycle = (dash + gap).max(3.0);
    let mut phase = 0.0_f64;
    let mut emitted = 0_usize;
    for i in 0..seg_pairs {
        if emitted >= 256 {
            break;
        }
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        let dx = f64::from(b.x - a.x);
        let dy = f64::from(b.y - a.y);
        let len = dx.hypot(dy);
        if len <= 1e-3 || !len.is_finite() {
            continue;
        }
        let ux = dx / len;
        let uy = dy / len;
        let mut d = 0.0_f64;
        while d + 1e-3 < len && emitted < 256 {
            phase = phase.rem_euclid(cycle);
            if phase < dash - 1e-3 {
                let take = (dash - phase).min(len - d).max(0.0);
                let p0 = egui::pos2(a.x + (ux * d) as f32, a.y + (uy * d) as f32);
                let p1 = egui::pos2(
                    a.x + (ux * (d + take)) as f32,
                    a.y + (uy * (d + take)) as f32,
                );
                rasterize_capsule_direct(pixels, tex_size, p0, p1, radius, color);
                emitted += 1;
                let adv = take.max((len - d).min(0.1));
                d = (d + adv).min(len);
                phase = (phase + adv).rem_euclid(cycle);
                if (phase - dash).abs() <= 1e-3 {
                    phase = dash;
                }
            } else {
                let skip = (cycle - phase).min(len - d).max(0.0);
                let adv = skip.max((len - d).min(0.1));
                d = (d + adv).min(len);
                phase = (phase + adv).rem_euclid(cycle);
                if phase >= cycle - 1e-3 {
                    phase = 0.0;
                }
            }
        }
    }
}

fn rasterize_thumbnail_shapes(
    image: &mut egui::ColorImage,
    sheet_size: egui::Vec2,
    page: &NotebookPage,
    theme: drawcraft_doc::UiTheme,
    auto_contrast: bool,
) {
    if page.shapes.is_empty() {
        return;
    }
    let tex_size = image.size;
    let sx_img = (tex_size[0] as f32) / page.width.max(1) as f32;
    let sy_img = (tex_size[1] as f32) / page.height.max(1) as f32;
    let sx_sheet = sheet_size.x / page.width.max(1) as f32;
    let sy_sheet = sheet_size.y / page.height.max(1) as f32;
    let scale_img = sx_img.min(sy_img);
    let _ = (sx_sheet, sy_sheet);
    let (eff_contrast, eff_paper) = if page.pdf_backdrop.is_some() {
        (false, drawcraft_doc::PaperColor::PureWhite)
    } else {
        (auto_contrast, page.paper_color)
    };
    for item in &page.shapes {
        let shape = item.as_ref().sanitized();
        let color =
            to_color32(eff_paper.resolve_ink_color(theme, eff_contrast, shape.color, false));
        let fill = shape
            .fill_color
            .map(|fc| to_color32(eff_paper.resolve_ink_color(theme, eff_contrast, fc, false)));
        let radius = ((shape.width * scale_img).clamp(1.0, 4.0) * 0.5).max(0.65);
        for (points, closed) in shape.path_segments() {
            let pts = decimate_thumb_img_points(&points, sx_img, sy_img);
            if pts.len() < 2 {
                continue;
            }
            if closed && let Some(fc) = fill {
                rasterize_polygon_fill(&mut image.pixels, tex_size, &pts, fc);
            }
            rasterize_thumb_polyline(
                &mut image.pixels,
                tex_size,
                &pts,
                closed,
                radius,
                color,
                shape.line_style,
            );
        }
    }
}

struct ActivePageFooterCtx<'a> {
    active_idx: usize,
    page_count: usize,
    active_page: Option<&'a NotebookPage>,
    expand_t: f32,
    natural_drawer_h: f32,
}

/// Returns true when a pointer press or touch contact begins outside `rect`.
fn pointer_pressed_outside_rect(ctx: &egui::Context, rect: egui::Rect) -> bool {
    if !rect.is_positive() {
        return false;
    }
    ctx.input(|i| {
        let event_outside = i.events.iter().any(|ev| match ev {
            egui::Event::PointerButton {
                pos, pressed: true, ..
            }
            | egui::Event::Touch {
                phase: egui::TouchPhase::Start,
                pos,
                ..
            } => pos.x.is_finite() && pos.y.is_finite() && !rect.contains(*pos),
            _ => false,
        });
        let pointer_outside = i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .or_else(|| i.pointer.press_origin())
                .is_some_and(|p| p.x.is_finite() && p.y.is_finite() && !rect.contains(p));
        event_outside || pointer_outside
    })
}

/// Renders the docked active-page footer card, keeping navigation and Add Page compact
/// while expanding the paper summary and secondary page actions on demand.
fn draw_active_page_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    ctx: ActivePageFooterCtx<'_>,
    t: &Tokens,
) -> FooterLayout {
    let was_expanded = app.view.pages_footer_expanded;
    let total_pages = ctx.page_count.max(1);
    let page_num = (ctx.active_idx + 1).min(total_pages);
    let mut prev_rect = egui::Rect::NOTHING;
    let mut next_rect = egui::Rect::NOTHING;
    let mut toggle_rect = egui::Rect::NOTHING;
    let mut measured_drawer_h = None;
    let mut cur_drawer_h = 0.0_f32;

    let footer_rect = card_frame(t)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;

            ui.horizontal(|ui| {
                ui.set_height(28.0);
                ui.label(
                    egui::RichText::new(format!("Page {page_num}"))
                        .size(13.0)
                        .strong()
                        .color(t.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (pr, nr) = draw_page_stepper(app, ui, ctx.active_idx, total_pages, t);
                    prev_rect = pr;
                    next_rect = nr;
                });
            });

            if ctx.expand_t > 0.001 {
                cur_drawer_h = (ctx.natural_drawer_h * ctx.expand_t).max(0.0);
                let avail_w = ui.available_width();
                let (slot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(avail_w, cur_drawer_h), egui::Sense::hover());
                let child_max = egui::Rect::from_min_size(
                    slot_rect.min,
                    egui::vec2(avail_w, ctx.natural_drawer_h.max(cur_drawer_h) + 64.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(child_max)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                child.set_clip_rect(slot_rect.intersect(ui.clip_rect()));
                child.set_width(avail_w);
                child.spacing_mut().item_spacing.y = 0.0;
                child.multiply_opacity(crate::motion::smoothstep(ctx.expand_t));
                if !app.view.pages_footer_expanded {
                    // Prevent stray clicks on secondary buttons while the drawer animates shut.
                    child.disable();
                }
                draw_footer_expandable_options(
                    app,
                    &mut child,
                    ctx.active_idx,
                    total_pages,
                    page_num,
                    ctx.active_page,
                    t,
                );
                measured_drawer_h = Some(child.min_rect().height().max(0.0));
            }

            ui.add_space(6.0);
            toggle_rect = draw_footer_primary_bar(app, ui, page_num, t);
        })
        .response
        .rect;

    if was_expanded
        && app.view.pages_footer_expanded
        && pointer_pressed_outside_rect(ui.ctx(), footer_rect)
    {
        app.view.pages_footer_expanded = false;
        ui.ctx().request_repaint();
    }

    FooterLayout {
        footer_rect,
        prev_rect,
        next_rect,
        toggle_rect,
        measured_drawer_h,
        cur_drawer_h,
    }
}

/// Renders the expandable paper details and secondary active-page actions.
fn draw_footer_expandable_options(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    active_idx: usize,
    total_pages: usize,
    page_num: usize,
    active_page: Option<&NotebookPage>,
    t: &Tokens,
) {
    if let Some(page) = active_page {
        ui.add_space(3.0);
        draw_active_paper_summary(app, ui, page, t);
    }

    ui.add_space(5.0);
    draw_card_hairline(ui, t);
    ui.add_space(5.0);

    let btn_h = 26.0;
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.columns(2, |cols| {
        if sidebar_action_button(
            &mut cols[0],
            Icon::Paper,
            "Insert PDF",
            ActionTone::Neutral,
            true,
            btn_h,
            t,
        )
        .on_hover_text(format!(
            "Browse for a PDF file and insert its pages after Page {page_num}"
        ))
        .clicked()
        {
            app.trigger_import_pdf_dialog(crate::PdfImportTarget::InsertIntoActive);
        }

        if sidebar_action_button(
            &mut cols[1],
            Icon::Duplicate,
            "Duplicate",
            ActionTone::Neutral,
            true,
            btn_h,
            t,
        )
        .on_hover_text(format!("Create a copy of Page {page_num} right after it"))
        .clicked()
        {
            crate::canvas::commit_pending_text(app);
            let _ = app.session.execute("page.duplicate", &json!({}));
            app.mesh_cache.clear();
            cols[1].ctx().request_repaint();
        }
    });

    ui.add_space(4.0);
    ui.columns(2, |cols| {
        if sidebar_action_button(
            &mut cols[0],
            Icon::RotateCw,
            "Rotate 90°",
            ActionTone::Neutral,
            true,
            btn_h,
            t,
        )
        .on_hover_text(format!("Rotate Page {page_num} 90° clockwise"))
        .clicked()
        {
            crate::canvas::commit_pending_text(app);
            let _ = app
                .session
                .execute("page.rotate", &json!({ "quarter_turns": 1 }));
            app.mesh_cache.clear();
            app.view.fit_requested = true;
        }

        if sidebar_action_button(
            &mut cols[1],
            Icon::Transfer,
            "Transfer",
            ActionTone::Neutral,
            true,
            btn_h,
            t,
        )
        .on_hover_text(format!(
            "Copy or move Page {page_num} to another or new notebook"
        ))
        .clicked()
        {
            app.view.transfer_pages_list = vec![active_idx];
            app.view.transfer_merge_whole_notebook = None;
            modal::open(app, ActiveModal::TransferPages);
        }
    });

    ui.add_space(4.0);
    let can_remove = total_pages > 1;
    let remove_tip = if can_remove {
        format!("Delete Page {page_num} from this notebook")
    } else {
        "A notebook must keep at least one page".to_string()
    };
    if sidebar_action_button(
        ui,
        Icon::Trash,
        "Remove",
        ActionTone::Danger,
        can_remove,
        btn_h,
        t,
    )
    .on_hover_text(remove_tip)
    .clicked()
    {
        crate::canvas::commit_pending_text(app);
        let _ = app.session.execute("page.remove", &json!({}));
        app.mesh_cache.clear();
        app.view.fit_requested = true;
        ui.ctx().request_repaint();
    }

    ui.add_space(5.0);
    draw_card_hairline(ui, t);
}

/// Renders the persistent bottom row containing Add Page and the expand/collapse chevron.
fn draw_footer_primary_bar(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    page_num: usize,
    t: &Tokens,
) -> egui::Rect {
    let btn_h = 26.0;
    let toggle_w = 28.0;
    let gap = 6.0;
    let total_w = ui.available_width().max(toggle_w + gap + 48.0);
    let add_w = (total_w - toggle_w - gap).max(48.0);
    let mut toggle_rect = egui::Rect::NOTHING;

    ui.horizontal(|ui| {
        ui.set_height(btn_h);
        ui.spacing_mut().item_spacing.x = gap;
        ui.allocate_ui_with_layout(
            egui::vec2(add_w, btn_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if sidebar_action_button(
                    ui,
                    Icon::AddPage,
                    "Add Page",
                    ActionTone::Accent,
                    true,
                    btn_h,
                    t,
                )
                .on_hover_text(format!("Insert a new blank page after Page {page_num}"))
                .clicked()
                {
                    crate::canvas::commit_pending_text(app);
                    let _ = app.session.execute("page.add", &json!({}));
                    app.mesh_cache.clear();
                    ui.ctx().request_repaint();
                }
            },
        );

        let toggle_resp =
            draw_footer_toggle_button(ui, app.view.pages_footer_expanded, toggle_w, btn_h, t);
        toggle_rect = toggle_resp.rect;
        if toggle_resp.clicked() {
            app.view.pages_footer_expanded = !app.view.pages_footer_expanded;
            ui.ctx().request_repaint();
        }
    });

    toggle_rect
}

/// Renders the chevron toggle button that expands or collapses extra footer options.
fn draw_footer_toggle_button(
    ui: &mut egui::Ui,
    expanded: bool,
    width: f32,
    height: f32,
    t: &Tokens,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fg_c = paint_footer_pill_box(ui, rect, &resp, ActionTone::Neutral, true, t);
        let icon = if expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronUp
        };
        let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(14.5, 14.5));
        paint_icon(ui.painter(), icon_rect, icon, fg_c);
    }
    let tip = if expanded {
        "Hide extra page options"
    } else {
        "Show more page options"
    };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
}

fn step_page_button(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    icon: Icon,
    enabled: bool,
    target_idx: usize,
    tooltip: &str,
    t: &Tokens,
) -> egui::Rect {
    let resp = icon_button(ui, icon, None, false, enabled, t).on_hover_text(tooltip);
    let resp = if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    let rect = resp.rect;
    if resp.clicked() {
        crate::canvas::commit_pending_text(app);
    }
    if resp.clicked()
        && app
            .session
            .execute("page.select", &json!({ "index": target_idx }))
            .is_ok()
    {
        app.mesh_cache.clear();
        ui.ctx().request_repaint();
    }
    rect
}

fn draw_page_stepper(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    active_idx: usize,
    total_pages: usize,
    t: &Tokens,
) -> (egui::Rect, egui::Rect) {
    ui.spacing_mut().item_spacing.x = 4.0;
    let next_rect = step_page_button(
        app,
        ui,
        Icon::NextPage,
        active_idx + 1 < total_pages,
        active_idx + 1,
        "Next page (PageDown)",
        t,
    );
    draw_page_counter_pill(ui, active_idx + 1, total_pages, t)
        .on_hover_text(format!("Viewing page {} of {total_pages}", active_idx + 1));
    let prev_rect = step_page_button(
        app,
        ui,
        Icon::PrevPage,
        active_idx > 0,
        active_idx.saturating_sub(1),
        "Previous page (PageUp)",
        t,
    );
    (prev_rect, next_rect)
}

fn draw_page_counter_pill(
    ui: &mut egui::Ui,
    current: usize,
    total: usize,
    t: &Tokens,
) -> egui::Response {
    let label = format!("{current} / {total}");
    let font = egui::FontId::monospace(11.5);
    let galley = ui.painter().layout_no_wrap(label, font, t.text);
    let width = (galley.size().x + 14.0).max(44.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, egui::CornerRadius::same(7), t.toolbar_bg);
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(7),
            egui::Stroke::new(1.0, t.border),
            egui::StrokeKind::Inside,
        );
        let pos = egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        );
        p.galley(pos, galley, t.text);
    }
    resp
}

fn draw_active_paper_summary(
    app: &DrawCraftApp,
    ui: &mut egui::Ui,
    page: &NotebookPage,
    t: &Tokens,
) {
    let (eff_contrast, eff_paper) = crate::effective_page_ink_context(page, &app.session.prefs);
    let paper_colors = eff_paper.resolved_colors(app.session.prefs.theme, eff_contrast);
    let summary_text = if let Some(backdrop) = page.pdf_backdrop {
        format!("PDF Backdrop • PDF page {}", backdrop.page_index + 1)
    } else {
        format!(
            "{} • {}",
            page.paper_template.label(),
            page.paper_color.label()
        )
    };
    ui.horizontal(|ui| {
        ui.set_height(15.0);
        ui.spacing_mut().item_spacing.x = 6.0;
        let (swatch_rect, _) = ui.allocate_exact_size(egui::vec2(11.0, 11.0), egui::Sense::hover());
        if ui.is_rect_visible(swatch_rect) {
            let p = ui.painter();
            p.rect_filled(
                swatch_rect,
                egui::CornerRadius::same(3),
                to_color32(paper_colors.background),
            );
            p.rect_stroke(
                swatch_rect,
                egui::CornerRadius::same(3),
                egui::Stroke::new(1.0, t.border),
                egui::StrokeKind::Inside,
            );
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(summary_text.clone())
                    .size(11.0)
                    .color(t.text_muted),
            )
            .truncate(),
        )
        .on_hover_text(summary_text);
    });
}

fn draw_card_hairline(ui: &mut egui::Ui, t: &Tokens) {
    let width = ui.available_width().max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().line_segment(
            [rect.left_center(), rect.right_center()],
            egui::Stroke::new(1.0, t.border.gamma_multiply(0.75)),
        );
    }
}

#[derive(Clone, Copy)]
enum ActionTone {
    Surface,
    Neutral,
    Accent,
    Selected,
    Danger,
}

fn footer_pill_colors(
    tone: ActionTone,
    enabled: bool,
    hover_t: f32,
    t: &Tokens,
) -> (Color32, Color32, f32, Color32) {
    if !enabled {
        if matches!(tone, ActionTone::Danger) {
            return (
                Color32::TRANSPARENT,
                Color32::TRANSPARENT,
                0.0,
                t.text_muted.gamma_multiply(0.55),
            );
        }
        return (
            t.toolbar_bg.gamma_multiply(0.75),
            t.border.gamma_multiply(0.6),
            1.0,
            t.text_muted.gamma_multiply(0.55),
        );
    }
    match tone {
        ActionTone::Surface => (
            crate::motion::lerp_color(t.field_bg, t.border.gamma_multiply(0.35), hover_t),
            crate::motion::lerp_color(t.border, t.text_muted.gamma_multiply(0.6), hover_t),
            1.0,
            t.text,
        ),
        ActionTone::Neutral => (
            crate::motion::lerp_color(t.toolbar_bg, t.border.gamma_multiply(0.45), hover_t),
            crate::motion::lerp_color(t.border, t.text_muted.gamma_multiply(0.6), hover_t),
            1.0,
            t.text,
        ),
        ActionTone::Accent => (
            crate::motion::lerp_color(t.accent_soft, t.accent.gamma_multiply(0.22), hover_t),
            crate::motion::lerp_color(t.accent.gamma_multiply(0.55), t.accent, hover_t),
            1.0,
            t.accent,
        ),
        ActionTone::Selected => (
            crate::motion::lerp_color(t.accent_soft, t.accent.gamma_multiply(0.22), hover_t),
            t.accent,
            1.5,
            t.accent,
        ),
        ActionTone::Danger => (
            Color32::TRANSPARENT,
            Color32::TRANSPARENT,
            0.0,
            crate::motion::lerp_color(t.danger_fg, t.text, hover_t * 0.18),
        ),
    }
}

fn paint_footer_pill_box(
    ui: &egui::Ui,
    rect: egui::Rect,
    resp: &egui::Response,
    tone: ActionTone,
    enabled: bool,
    t: &Tokens,
) -> Color32 {
    let hover_t =
        crate::motion::hover_factor(ui.ctx(), resp.id.with("hov"), enabled && resp.hovered());
    let (fill, stroke_c, stroke_w, fg_c) = footer_pill_colors(tone, enabled, hover_t, t);
    let p = ui.painter();
    if fill != Color32::TRANSPARENT {
        p.rect_filled(rect, egui::CornerRadius::same(7), fill);
    }
    if stroke_w > 0.0 && stroke_c != Color32::TRANSPARENT {
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(7),
            egui::Stroke::new(stroke_w, stroke_c),
            egui::StrokeKind::Inside,
        );
    }
    fg_c
}

fn sidebar_action_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    tone: ActionTone,
    enabled: bool,
    height: f32,
    t: &Tokens,
) -> egui::Response {
    let width = ui.available_width().max(48.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), sense);
    if ui.is_rect_visible(rect) {
        let fg_c = paint_footer_pill_box(ui, rect, &resp, tone, enabled, t);
        let p = ui.painter();

        let icon_dim = 13.5_f32;
        let gap = 5.0_f32;
        let max_text_w = (rect.width() - 12.0 - icon_dim - gap).max(16.0);
        let mut job = egui::text::LayoutJob::single_section(
            label.to_string(),
            egui::TextFormat {
                font_id: egui::FontId::proportional(11.5),
                color: fg_c,
                ..Default::default()
            },
        );
        job.wrap.max_width = max_text_w;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = p.layout_job(job);
        let content_w = icon_dim + gap + galley.size().x;
        let start_x = (rect.center().x - content_w * 0.5).max(rect.left() + 6.0);
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(start_x + icon_dim * 0.5, rect.center().y),
            egui::vec2(icon_dim, icon_dim),
        );
        paint_icon(p, icon_rect, icon, fg_c);
        p.galley(
            egui::pos2(
                icon_rect.right() + gap,
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            fg_c,
        );
    }

    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_sidebar_frames(app: &mut DrawCraftApp, frames: usize) -> SidebarMetrics {
        let ctx = egui::Context::default();
        let mut metrics = SidebarMetrics::default();

        for _ in 0..frames {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                metrics = show_inner(app, ui);
            });
            out.drop_without_applying_deltas();
        }

        metrics
    }

    #[test]
    fn active_page_footer_docks_at_bottom_of_sidebar() {
        let mut app = DrawCraftApp::default();
        let m = run_sidebar_frames(&mut app, 3);

        assert!(
            m.footer_rect.bottom() >= 775.0 && m.footer_rect.bottom() <= 795.0,
            "expected footer docked near bottom of 800px sidebar, got {:?}",
            m.footer_rect
        );
        assert!(
            !app.view.pages_footer_expanded,
            "active page footer should start collapsed by default"
        );
        assert!(
            m.footer_rect.height() >= 70.0 && m.footer_rect.height() <= 95.0,
            "expected compact collapsed docked footer card, got height {} ({:?})",
            m.footer_rect.height(),
            m.footer_rect
        );
        assert!(
            m.footer_rect.contains(m.footer_toggle_rect.center()),
            "expand/collapse toggle {:?} should lie inside footer {:?}",
            m.footer_toggle_rect,
            m.footer_rect
        );
        assert!(
            m.scroll_rect.top() >= m.pages_header_rect.bottom() + 4.0,
            "scroll area {:?} overlapped pages header {:?}",
            m.scroll_rect,
            m.pages_header_rect
        );
        assert!(
            m.footer_rect.top() >= m.scroll_rect.bottom() + 6.0,
            "footer {:?} overlapped scroll area {:?}",
            m.footer_rect,
            m.scroll_rect
        );
        assert!(
            m.first_tile_rect.top() >= m.scroll_rect.top() - 0.5
                && m.first_tile_rect.bottom() <= m.scroll_rect.bottom() + 0.5
                && m.first_tile_rect.height() >= 100.0,
            "first page tile {:?} was clipped inside scroll area {:?}",
            m.first_tile_rect,
            m.scroll_rect
        );
        assert!(
            (m.panel_rect.width() - DEFAULT_SIDEBAR_WIDTH).abs() <= 0.5,
            "sidebar width {:?} differed from fixed default {DEFAULT_SIDEBAR_WIDTH}",
            m.panel_rect
        );
    }

    #[test]
    fn many_pages_scroll_cleanly_between_header_and_docked_footer() {
        let mut app = DrawCraftApp::default();
        for _ in 0..11 {
            let _ = app.session.execute("page.add", &json!({}));
        }
        let m = run_sidebar_frames(&mut app, 3);

        assert!(
            m.footer_rect.bottom() >= 775.0 && m.footer_rect.bottom() <= 795.0,
            "expected footer docked near bottom with 12 pages, got {:?}",
            m.footer_rect
        );
        assert!(
            m.footer_rect.height() >= 70.0 && m.footer_rect.height() <= 95.0,
            "expected compact collapsed docked footer height with 12 pages, got {:?}",
            m.footer_rect
        );
        assert!(
            m.scroll_rect.top() >= m.pages_header_rect.bottom() + 4.0,
            "scroll area {:?} overlapped pages header {:?}",
            m.scroll_rect,
            m.pages_header_rect
        );
        assert!(
            m.footer_rect.top() >= m.scroll_rect.bottom() + 6.0,
            "footer {:?} overlapped scroll area {:?}",
            m.footer_rect,
            m.scroll_rect
        );
        assert!(
            m.first_tile_rect.height() >= 100.0,
            "first page tile {:?} unexpectedly squished",
            m.first_tile_rect
        );
    }

    #[test]
    fn sidebar_with_toolbar_aligns_on_frame_zero_and_steady_state() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();

        for frame_idx in 0..3 {
            let mut m = SidebarMetrics::default();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                crate::toolbar::show(&mut app, ui);
                m = show_inner(&mut app, ui);
            });
            out.drop_without_applying_deltas();

            assert!(
                m.scroll_rect.top() >= m.pages_header_rect.bottom() + 4.0,
                "frame {frame_idx}: scroll {:?} overlapped pages header {:?}",
                m.scroll_rect,
                m.pages_header_rect
            );
            assert!(
                m.footer_rect.top() >= m.scroll_rect.bottom() + 4.0,
                "frame {frame_idx}: footer {:?} overlapped scroll {:?}",
                m.footer_rect,
                m.scroll_rect
            );
            assert!(
                m.footer_rect.bottom() <= m.panel_rect.bottom() && m.footer_rect.bottom() >= 770.0,
                "frame {frame_idx}: footer {:?} not cleanly inside bottom of panel {:?}",
                m.footer_rect,
                m.panel_rect
            );
            assert!(
                m.first_tile_rect.top() >= m.scroll_rect.top() - 0.5
                    && m.first_tile_rect.bottom() <= m.scroll_rect.bottom() + 0.5
                    && m.first_tile_rect.height() >= 100.0,
                "frame {frame_idx}: first tile {:?} clipped in scroll {:?}",
                m.first_tile_rect,
                m.scroll_rect
            );
        }
    }

    #[test]
    fn notebook_settings_button_opens_notebook_settings_modal() {
        let mut app = DrawCraftApp::default();
        app.view.inspector_open = false;
        app.view.settings_tab = crate::SettingsTab::Pen;

        let ctx = egui::Context::default();
        let mut btn_center = None;
        let mut pages_title_right = 0.0_f32;
        let mut btn_text_left = 0.0_f32;
        let mut sidebar_metrics = SidebarMetrics::default();
        let mut header_badge_labels = Vec::new();
        for _ in 0..2 {
            header_badge_labels.clear();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                sidebar_metrics = show_inner(&mut app, ui);
            });
            for clipped in &out.shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    let label = text.galley.text();
                    assert_ne!(label, "+ New Book");
                    assert_ne!(label, "Paper");
                    let text_rect = text.galley.rect.translate(text.pos.to_vec2());
                    if label == "Pages" {
                        pages_title_right = text_rect.right();
                    }
                    if label == "1"
                        && sidebar_metrics
                            .pages_header_rect
                            .contains(text_rect.center())
                    {
                        header_badge_labels.push(label.to_string());
                    }
                    if label == "Notebook Settings" {
                        btn_text_left = text_rect.left();
                        assert!(
                            text_rect.right() <= sidebar_metrics.panel_rect.right(),
                            "Notebook Settings label clipped outside sidebar"
                        );
                        btn_center = Some(text_rect.center());
                    }
                }
            }
            out.drop_without_applying_deltas();
        }
        assert!(
            header_badge_labels.is_empty(),
            "unexpected page count badge in Pages header: {header_badge_labels:?}"
        );
        assert!(
            pages_title_right < btn_text_left - 8.0,
            "Pages title ({pages_title_right}) overlapped Notebook Settings ({btn_text_left})"
        );
        let pos = btn_center.expect("Notebook Settings button label should be painted");
        for (pressed, time) in [(true, 0.1), (false, 0.15)] {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(time),
                events: vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                let _ = show_inner(&mut app, ui);
            });
            out.drop_without_applying_deltas();
        }

        assert!(!app.view.inspector_open);
        assert_eq!(app.view.active_modal, Some(ActiveModal::NotebookSettings));
    }

    fn find_sidebar_label_center(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        target: &str,
    ) -> (SidebarMetrics, Option<egui::Pos2>, Vec<String>) {
        let mut metrics = SidebarMetrics::default();
        let mut center = None;
        let mut labels = Vec::new();
        for _ in 0..2 {
            labels.clear();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                metrics = show_inner(app, ui);
            });
            for clipped in &out.shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    let s = text.galley.text().to_string();
                    if s == target {
                        center = Some(text.pos + text.galley.rect.center().to_vec2());
                    }
                    labels.push(s);
                }
            }
            out.drop_without_applying_deltas();
        }
        (metrics, center, labels)
    }

    fn click_sidebar_pos(ctx: &egui::Context, app: &mut DrawCraftApp, pos: egui::Pos2, t0: f64) {
        for (pressed, time) in [(true, t0), (false, t0 + 0.05)] {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(time),
                events: vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                let _ = show_inner(app, ui);
            });
            out.drop_without_applying_deltas();
        }
    }

    fn capture_sidebar_text_rects_at(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        start_time: f64,
        frames: usize,
        dt: f64,
    ) -> (SidebarMetrics, Vec<(String, egui::Rect)>) {
        let mut metrics = SidebarMetrics::default();
        let mut rects = Vec::new();
        for step in 0..frames {
            rects.clear();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(start_time + (step as f64) * dt),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                metrics = show_inner(app, ui);
            });
            for clipped in &out.shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    rects.push((text.galley.text().to_string(), rect));
                }
            }
            out.drop_without_applying_deltas();
        }
        (metrics, rects)
    }

    #[test]
    fn footer_page_controls_navigate_add_duplicate_and_remove_pages() {
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));
        assert_eq!(app.session.doc.page_count(), 2);
        assert_eq!(app.session.doc.active_page_index(), 0);

        let ctx = egui::Context::default();
        let (m, rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.0, 3, 0.025);
        assert!(rects.iter().any(|(s, _)| s == "1 / 2"), "rects: {rects:?}");
        assert!(
            rects.iter().any(|(s, _)| s == "Add Page"),
            "rects: {rects:?}"
        );
        assert!(
            !rects.iter().any(|(s, _)| s == "Duplicate"),
            "secondary options should stay collapsed initially: {rects:?}"
        );
        assert!(
            !rects.iter().any(|(s, _)| s == "Remove"),
            "rects: {rects:?}"
        );
        assert!(
            !rects.iter().any(|(s, _)| s == "+ Page"),
            "rects: {rects:?}"
        );

        // Next page button steps to Page 2.
        click_sidebar_pos(&ctx, &mut app, m.next_btn_rect.center(), 0.1);
        assert_eq!(app.session.doc.active_page_index(), 1);

        // Prev page button steps back to Page 1.
        let (m, rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.2, 3, 0.025);
        assert!(rects.iter().any(|(s, _)| s == "2 / 2"), "rects: {rects:?}");
        click_sidebar_pos(&ctx, &mut app, m.prev_btn_rect.center(), 0.3);
        assert_eq!(app.session.doc.active_page_index(), 0);

        // Add Page inserts a new page even while the footer is collapsed.
        let (_, rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.4, 2, 0.025);
        let add_pos = text_rect(&rects, "Add Page").center();
        click_sidebar_pos(&ctx, &mut app, add_pos, 0.5);
        assert_eq!(app.session.doc.page_count(), 3);

        // Expand footer via the arrow toggle to access Duplicate and Remove.
        click_sidebar_pos(&ctx, &mut app, m.footer_toggle_rect.center(), 0.7);
        assert!(app.view.pages_footer_expanded);
        let (m_exp, exp_rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.8, 12, 0.025);
        assert!(exp_rects.iter().any(|(s, _)| s == "Insert PDF"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Duplicate"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Rotate 90°"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Transfer"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Remove"));

        // Duplicate copies the active page.
        let dup_pos = text_rect(&exp_rects, "Duplicate").center();
        click_sidebar_pos(&ctx, &mut app, dup_pos, 1.2);
        assert_eq!(app.session.doc.page_count(), 4);

        // Remove deletes active pages down to 1, where Remove becomes disabled.
        for step in 0..3 {
            let (_, rem_rects) =
                capture_sidebar_text_rects_at(&ctx, &mut app, 1.3 + f64::from(step) * 0.2, 2, 0.02);
            let rem_pos = text_rect(&rem_rects, "Remove").center();
            click_sidebar_pos(&ctx, &mut app, rem_pos, 1.35 + f64::from(step) * 0.2);
        }
        assert_eq!(app.session.doc.page_count(), 1);

        let (_, rem_rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 2.0, 2, 0.02);
        let rem_pos = text_rect(&rem_rects, "Remove").center();
        click_sidebar_pos(&ctx, &mut app, rem_pos, 2.1);
        assert_eq!(app.session.doc.page_count(), 1);

        // Clicking the toggle again collapses the extra options cleanly.
        click_sidebar_pos(&ctx, &mut app, m_exp.footer_toggle_rect.center(), 2.3);
        assert!(!app.view.pages_footer_expanded);
        let (_, collapsed_rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 2.4, 12, 0.025);
        assert!(!collapsed_rects.iter().any(|(s, _)| s == "Duplicate"));
        assert!(collapsed_rects.iter().any(|(s, _)| s == "Add Page"));
    }

    fn run_timed_sidebar_frames(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        start_time: f64,
        frames: usize,
        dt: f64,
        events_fn: impl Fn(usize) -> Vec<egui::Event>,
    ) -> SidebarMetrics {
        let mut metrics = SidebarMetrics::default();
        for step in 0..frames {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(start_time + (step as f64) * dt),
                events: events_fn(step),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                metrics = show_inner(app, ui);
            });
            out.drop_without_applying_deltas();
        }
        metrics
    }

    #[test]
    fn adding_and_duplicating_pages_scrolls_new_active_tile_into_view() {
        let mut app = DrawCraftApp::default();
        for _ in 0..11 {
            let _ = app.session.execute("page.add", &json!({}));
        }
        assert_eq!(app.session.doc.page_count(), 12);
        assert_eq!(app.session.doc.active_page_index(), 11);

        let ctx = egui::Context::default();
        let m0 = run_timed_sidebar_frames(&ctx, &mut app, 0.0, 20, 0.025, |_| Vec::new());
        assert!(
            m0.active_tile_rect.top() >= m0.scroll_rect.top() - 0.5
                && m0.active_tile_rect.bottom() <= m0.scroll_rect.bottom() + 0.5,
            "initial active tile {:?} should fit in scroll_rect {:?}",
            m0.active_tile_rect,
            m0.scroll_rect
        );

        // Click "+ Add Page" to append Page 13 on a new row below the current viewport.
        let (_, add_pos, _) = find_sidebar_label_center(&ctx, &mut app, "Add Page");
        click_sidebar_pos(&ctx, &mut app, add_pos.expect("Add Page button"), 0.6);
        assert_eq!(app.session.doc.page_count(), 13);
        assert_eq!(app.session.doc.active_page_index(), 12);

        let m_after_add = run_timed_sidebar_frames(&ctx, &mut app, 0.7, 24, 0.025, |_| Vec::new());
        assert!(
            m_after_add.active_tile_rect.top() >= m_after_add.scroll_rect.top() - 0.5
                && m_after_add.active_tile_rect.bottom() <= m_after_add.scroll_rect.bottom() + 0.5,
            "newly added Page 13 tile {:?} was not scrolled into scroll_rect {:?}",
            m_after_add.active_tile_rect,
            m_after_add.scroll_rect
        );

        // Expand footer to duplicate twice and push into another new row (Page 14 -> Page 15).
        click_sidebar_pos(
            &ctx,
            &mut app,
            m_after_add.footer_toggle_rect.center(),
            1.35,
        );
        let (_, exp_rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 1.45, 12, 0.025);
        let dup_pos = text_rect(&exp_rects, "Duplicate").center();
        for i in 0..2 {
            click_sidebar_pos(&ctx, &mut app, dup_pos, 1.8 + f64::from(i) * 0.15);
        }
        assert_eq!(app.session.doc.page_count(), 15);
        assert_eq!(app.session.doc.active_page_index(), 14);

        let m_after_dup = run_timed_sidebar_frames(&ctx, &mut app, 2.2, 24, 0.025, |_| Vec::new());
        assert!(
            m_after_dup.active_tile_rect.top() >= m_after_dup.scroll_rect.top() - 0.5
                && m_after_dup.active_tile_rect.bottom() <= m_after_dup.scroll_rect.bottom() + 0.5,
            "duplicated Page 15 tile {:?} was not scrolled into scroll_rect {:?}",
            m_after_dup.active_tile_rect,
            m_after_dup.scroll_rect
        );
    }

    #[test]
    fn manual_scroll_is_preserved_until_page_is_added_offscreen() {
        let mut app = DrawCraftApp::default();
        for _ in 0..13 {
            let _ = app.session.execute("page.add", &json!({}));
        }
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));

        let ctx = egui::Context::default();
        let m_top = run_timed_sidebar_frames(&ctx, &mut app, 0.0, 20, 0.025, |_| Vec::new());
        assert!(
            m_top.first_tile_rect.top() >= m_top.scroll_rect.top() - 0.5
                && m_top.first_tile_rect.bottom() <= m_top.scroll_rect.bottom() + 0.5,
            "Page 1 tile {:?} should start inside scroll_rect {:?}",
            m_top.first_tile_rect,
            m_top.scroll_rect
        );

        // Manually scroll down while Page 1 remains active.
        let wheel_pos = m_top.scroll_rect.center();
        let m_scrolled = run_timed_sidebar_frames(&ctx, &mut app, 0.6, 8, 0.025, |step| {
            if step < 4 {
                vec![
                    egui::Event::PointerMoved(wheel_pos),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -300.0),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            } else {
                Vec::new()
            }
        });
        assert!(
            m_scrolled.first_tile_rect.bottom() < m_scrolled.scroll_rect.top(),
            "manual wheel scroll should not be overridden while Page 1 stays selected: first={:?}, scroll={:?}",
            m_scrolled.first_tile_rect,
            m_scrolled.scroll_rect
        );

        // Add Page while scrolled away: inserts Page 2 off-screen above and must scroll back to highlight it.
        let (_, add_pos, _) = find_sidebar_label_center(&ctx, &mut app, "Add Page");
        click_sidebar_pos(&ctx, &mut app, add_pos.expect("Add Page button"), 1.0);
        assert_eq!(app.session.doc.active_page_index(), 1);

        let m_after_add = run_timed_sidebar_frames(&ctx, &mut app, 1.1, 24, 0.025, |_| Vec::new());
        assert!(
            m_after_add.active_tile_rect.top() >= m_after_add.scroll_rect.top() - 0.5
                && m_after_add.active_tile_rect.bottom() <= m_after_add.scroll_rect.bottom() + 0.5,
            "newly inserted Page 2 tile {:?} should scroll back into scroll_rect {:?}",
            m_after_add.active_tile_rect,
            m_after_add.scroll_rect
        );
    }

    #[test]
    fn sidebar_supports_bookmark_filter_and_batch_selection_controls() {
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute(
            "page.setBookmark",
            &json!({ "index": 1, "bookmarked": true }),
        );

        app.view.pages_bookmarked_only = true;
        app.view.pages_select_mode = true;
        app.view.selected_page_indices = vec![0, 1];

        let m = run_sidebar_frames(&mut app, 4);
        assert!(m.first_tile_rect.is_positive());
        assert!(m.footer_rect.top() > m.pages_header_rect.bottom());
    }

    #[test]
    fn virtualized_sidebar_handles_many_pages_and_jumps_to_active_tile() {
        let mut app = DrawCraftApp::default();
        let mut doc = drawcraft_doc::Document::new(600, 800);
        for _ in 1..600 {
            doc.add_page(None, None).expect("add page");
        }
        assert!(doc.select_page(550));
        app.session.doc = Arc::new(doc);

        let ctx = egui::Context::default();
        let m = run_timed_sidebar_frames(&ctx, &mut app, 0.0, 24, 0.025, |_| Vec::new());
        assert!(m.active_tile_rect.is_positive());
        assert!(
            m.active_tile_rect.top() >= m.scroll_rect.top() - 1.0
                && m.active_tile_rect.bottom() <= m.scroll_rect.bottom() + 1.0,
            "page 551 tile {:?} should scroll into scroll_rect {:?}",
            m.active_tile_rect,
            m.scroll_rect
        );
    }

    #[test]
    fn clicking_page_bookmark_icon_toggles_bookmark_without_selecting_tile() {
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        // Active page is Page 2 (index 1); Page 1 (index 0) is inactive.
        assert_eq!(app.session.doc.active_page_index(), 1);
        assert_eq!(app.session.doc.bookmarked_page_count(), 0);

        let ctx = egui::Context::default();
        let m = run_timed_sidebar_frames(&ctx, &mut app, 0.0, 4, 0.025, |_| Vec::new());
        assert!(m.first_bookmark_rect.is_positive());

        // Click bookmark icon on Page 1 (inactive): bookmarks Page 1 without switching away from Page 2.
        click_sidebar_pos(&ctx, &mut app, m.first_bookmark_rect.center(), 0.2);
        assert_eq!(app.session.doc.bookmarked_page_indices(), vec![0]);
        assert_eq!(app.session.doc.active_page_index(), 1);

        // Clicking it again un-bookmarks Page 1.
        click_sidebar_pos(&ctx, &mut app, m.first_bookmark_rect.center(), 0.4);
        assert_eq!(app.session.doc.bookmarked_page_count(), 0);
        assert_eq!(app.session.doc.active_page_index(), 1);

        // Switch to Page 1 (active) and click its bookmark icon: toggles active page bookmark cleanly.
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));
        let m_active = run_timed_sidebar_frames(&ctx, &mut app, 0.6, 4, 0.025, |_| Vec::new());
        click_sidebar_pos(&ctx, &mut app, m_active.first_bookmark_rect.center(), 0.8);
        assert_eq!(app.session.doc.bookmarked_page_indices(), vec![0]);
        assert!(app.session.doc.bookmarked);

        // Bookmarked icon renders with reddish danger tokens instead of selection blue.
        let tokens = crate::theme::Tokens::for_theme(app.session.prefs.theme);
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(0.9),
                ..Default::default()
            },
            |ui| {
                crate::theme::apply_visuals(ui.ctx(), app.session.prefs.theme);
                let _ = show_inner(&mut app, ui);
            },
        );
        fn has_stroke_color(shape: &egui::Shape, target: Color32) -> bool {
            match shape {
                egui::Shape::Rect(r) => r.stroke.color == target,
                egui::Shape::Vec(items) => items.iter().any(|s| has_stroke_color(s, target)),
                _ => false,
            }
        }
        assert!(
            out.shapes
                .iter()
                .any(|c| has_stroke_color(&c.shape, tokens.danger_border)),
            "expected reddish bookmark badge border when page is bookmarked"
        );
        out.drop_without_applying_deltas();

        // Clicking the thumbnail sheet itself still selects Page 1 when Page 2 is active.
        let _ = app.session.execute("page.select", &json!({ "index": 1 }));
        assert_eq!(app.session.doc.active_page_index(), 1);
        let m_jump = run_timed_sidebar_frames(&ctx, &mut app, 1.0, 4, 0.025, |_| Vec::new());
        click_sidebar_pos(&ctx, &mut app, m_jump.first_tile_rect.center(), 1.2);
        assert_eq!(app.session.doc.active_page_index(), 0);
    }

    fn capture_sidebar_text_rects(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
    ) -> (SidebarMetrics, Vec<(String, egui::Rect)>) {
        let mut metrics = SidebarMetrics::default();
        let mut rects = Vec::new();
        for _ in 0..3 {
            rects.clear();
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                metrics = show_inner(app, ui);
            });
            for clipped in &out.shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    rects.push((text.galley.text().to_string(), rect));
                }
            }
            out.drop_without_applying_deltas();
        }
        (metrics, rects)
    }

    fn text_rect(rects: &[(String, egui::Rect)], label: &str) -> egui::Rect {
        rects
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("expected label {label:?} in {rects:?}"))
    }

    #[test]
    fn header_controls_align_on_standardized_two_column_grid() {
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute(
            "page.setBookmark",
            &json!({ "index": 0, "bookmarked": true }),
        );

        let ctx = egui::Context::default();
        let (m, rects) = capture_sidebar_text_rects(&ctx, &mut app);
        let pages_r = text_rect(&rects, "Pages");
        let settings_r = text_rect(&rects, "Notebook Settings");
        let saved_r = text_rect(&rects, "Saved (1)");
        let select_r = text_rect(&rects, "Select");

        assert!(
            !rects
                .iter()
                .any(|(text, rect)| text == "2" && m.pages_header_rect.contains(rect.center())),
            "Pages header should not display the page count badge"
        );
        assert!((pages_r.center().y - settings_r.center().y).abs() <= 1.5);
        assert!(settings_r.left() > pages_r.right() + 16.0);

        assert!((saved_r.center().y - select_r.center().y).abs() <= 1.5);
        assert!(saved_r.top() > pages_r.bottom() + 4.0);
        assert!(select_r.left() > saved_r.right() + 16.0);
        assert!(m.pages_header_rect.contains(saved_r.center()));
        assert!(m.pages_header_rect.contains(select_r.center()));

        // Clicking Select switches the right header pill to Done on the same row.
        click_sidebar_pos(&ctx, &mut app, select_r.center(), 0.2);
        assert!(app.view.pages_select_mode);
        let (_, sel_rects) = capture_sidebar_text_rects(&ctx, &mut app);
        let done_r = text_rect(&sel_rects, "Done");
        let saved_after_r = text_rect(&sel_rects, "Saved (1)");
        assert!((done_r.center().y - saved_after_r.center().y).abs() <= 1.5);
    }

    #[test]
    fn footer_header_stepper_and_actions_never_overlap_with_many_pages() {
        let mut app = DrawCraftApp::default();
        let mut doc = drawcraft_doc::Document::new(600, 800);
        for _ in 1..200 {
            doc.add_page(None, None).expect("add page");
        }
        assert!(doc.select_page(176));
        app.session.doc = Arc::new(doc);

        let ctx = egui::Context::default();
        let (m, rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.0, 3, 0.025);
        let page_title_r = text_rect(&rects, "Page 177");
        let counter_r = text_rect(&rects, "177 / 200");
        assert!(
            page_title_r.right() + 12.0 <= m.prev_btn_rect.left(),
            "Page 177 label {:?} collided with prev stepper {:?}",
            page_title_r,
            m.prev_btn_rect
        );
        assert!(m.prev_btn_rect.right() < counter_r.left());
        assert!(counter_r.right() < m.next_btn_rect.left());

        let add_collapsed_r = text_rect(&rects, "Add Page");
        assert!((add_collapsed_r.center().y - m.footer_toggle_rect.center().y).abs() <= 1.5);
        assert!(add_collapsed_r.right() + 6.0 <= m.footer_toggle_rect.left());
        assert!(!rects.iter().any(|(s, _)| s == "Insert PDF"));

        // Expand footer and verify all secondary action rows and Add Page align without overlap.
        click_sidebar_pos(&ctx, &mut app, m.footer_toggle_rect.center(), 0.2);
        let (m_exp, exp_rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.3, 12, 0.025);
        let add_r = text_rect(&exp_rects, "Add Page");
        let pdf_r = text_rect(&exp_rects, "Insert PDF");
        let dup_r = text_rect(&exp_rects, "Duplicate");
        let rot_r = text_rect(&exp_rects, "Rotate 90°");
        let trn_r = text_rect(&exp_rects, "Transfer");
        let rem_r = text_rect(&exp_rects, "Remove");

        assert!((pdf_r.center().y - dup_r.center().y).abs() <= 1.5);
        assert!((rot_r.center().y - trn_r.center().y).abs() <= 1.5);
        assert!(rot_r.top() > pdf_r.bottom() + 8.0);
        assert!(rem_r.top() > rot_r.bottom() + 8.0);
        assert!(add_r.top() > rem_r.bottom() + 10.0);
        assert!((add_r.center().y - m_exp.footer_toggle_rect.center().y).abs() <= 1.5);
        assert!(m_exp.footer_rect.contains(pdf_r.center()));
        assert!(m_exp.footer_rect.contains(add_r.center()));

        let (w_before, h_before) = app.session.doc.page_dimensions(176).expect("page dims");
        click_sidebar_pos(&ctx, &mut app, rot_r.center(), 0.7);
        let (w_after, h_after) = app.session.doc.page_dimensions(176).expect("page dims");
        assert_eq!((w_after, h_after), (h_before, w_before));
    }

    #[test]
    fn active_page_footer_expands_and_collapses_smoothly_keeping_toggle_anchored() {
        let mut app = DrawCraftApp::default();
        let ctx = egui::Context::default();

        let (m_collapsed, collapsed_rects) =
            capture_sidebar_text_rects_at(&ctx, &mut app, 0.0, 4, 0.025);
        assert!(!app.view.pages_footer_expanded);
        assert!(
            m_collapsed.footer_rect.height() >= 70.0 && m_collapsed.footer_rect.height() <= 90.0,
            "unexpected collapsed footer height {:?}",
            m_collapsed.footer_rect
        );
        assert!(!collapsed_rects.iter().any(|(s, _)| s == "Insert PDF"));

        // Click the arrow toggle button to expand.
        let toggle_center_collapsed = m_collapsed.footer_toggle_rect.center();
        click_sidebar_pos(&ctx, &mut app, toggle_center_collapsed, 0.2);
        assert!(app.view.pages_footer_expanded);

        // Verify intermediate frames animate height smoothly without clipping or scroll overlap.
        let mut prev_h = m_collapsed.footer_rect.height();
        let mut saw_mid = false;
        for step in 1..=10 {
            let m_step = run_timed_sidebar_frames(
                &ctx,
                &mut app,
                0.25 + f64::from(step) * 0.025,
                1,
                0.025,
                |_| Vec::new(),
            );
            assert!(
                m_step.footer_rect.bottom() <= m_step.panel_rect.bottom() + 0.5,
                "step {step}: footer {:?} clipped below panel {:?}",
                m_step.footer_rect,
                m_step.panel_rect
            );
            assert!(
                m_step.footer_rect.top() >= m_step.scroll_rect.bottom() + 4.0,
                "step {step}: footer {:?} overlapped scroll area {:?}",
                m_step.footer_rect,
                m_step.scroll_rect
            );
            let h = m_step.footer_rect.height();
            assert!(
                h >= prev_h - 0.5,
                "height regressed during expand: {h} < {prev_h}"
            );
            if h > m_collapsed.footer_rect.height() + 10.0 && step < 8 {
                saw_mid = true;
            }
            prev_h = h;
        }
        assert!(saw_mid, "expected smooth intermediate expansion heights");

        let (m_expanded, exp_rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.6, 4, 0.025);
        assert!(
            m_expanded.footer_rect.height() > m_collapsed.footer_rect.height() + 90.0,
            "expanded footer {:?} should be noticeably taller than collapsed {:?}",
            m_expanded.footer_rect,
            m_collapsed.footer_rect
        );
        assert!(exp_rects.iter().any(|(s, _)| s == "Insert PDF"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Duplicate"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Rotate 90°"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Transfer"));
        assert!(exp_rects.iter().any(|(s, _)| s == "Remove"));

        // Toggle stays anchored at the bottom so clicking again collapses without chasing it.
        assert!(
            (m_expanded.footer_toggle_rect.center() - toggle_center_collapsed).length() <= 1.0,
            "toggle shifted from {:?} to {:?}",
            toggle_center_collapsed,
            m_expanded.footer_toggle_rect.center()
        );

        click_sidebar_pos(&ctx, &mut app, m_expanded.footer_toggle_rect.center(), 0.8);
        assert!(!app.view.pages_footer_expanded);
        let (m_recollapsed, recollapsed_rects) =
            capture_sidebar_text_rects_at(&ctx, &mut app, 0.9, 12, 0.025);
        assert!(
            (m_recollapsed.footer_rect.height() - m_collapsed.footer_rect.height()).abs() <= 0.5,
            "re-collapsed height {:?} != initial {:?}",
            m_recollapsed.footer_rect,
            m_collapsed.footer_rect
        );
        assert!(!recollapsed_rects.iter().any(|(s, _)| s == "Insert PDF"));
    }

    #[test]
    fn active_page_footer_collapses_when_clicking_outside_footer_card() {
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        assert_eq!(app.session.doc.page_count(), 2);
        assert_eq!(app.session.doc.active_page_index(), 1);

        let ctx = egui::Context::default();
        let (m_collapsed, _) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.0, 4, 0.025);
        assert!(!app.view.pages_footer_expanded);

        // Expand footer via the chevron toggle.
        click_sidebar_pos(&ctx, &mut app, m_collapsed.footer_toggle_rect.center(), 0.2);
        assert!(app.view.pages_footer_expanded);
        let (_, exp_rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.3, 12, 0.025);
        assert!(exp_rects.iter().any(|(s, _)| s == "Duplicate"));

        // Clicking an action inside the expanded footer keeps it open.
        let rot_pos = text_rect(&exp_rects, "Rotate 90°").center();
        click_sidebar_pos(&ctx, &mut app, rot_pos, 0.7);
        assert!(app.view.pages_footer_expanded);

        // Clicking outside the sidebar (on the canvas) collapses the footer cleanly.
        click_sidebar_pos(&ctx, &mut app, egui::pos2(800.0, 400.0), 0.9);
        assert!(!app.view.pages_footer_expanded);
        let (m_recollapsed, recollapsed_rects) =
            capture_sidebar_text_rects_at(&ctx, &mut app, 1.0, 12, 0.025);
        assert!(
            (m_recollapsed.footer_rect.height() - m_collapsed.footer_rect.height()).abs() <= 0.5,
            "re-collapsed height {:?} != initial {:?}",
            m_recollapsed.footer_rect,
            m_collapsed.footer_rect
        );
        assert!(!recollapsed_rects.iter().any(|(s, _)| s == "Insert PDF"));

        // Re-expand and click a page thumbnail inside the sidebar above the footer:
        // collapses the footer while still selecting Page 1.
        click_sidebar_pos(
            &ctx,
            &mut app,
            m_recollapsed.footer_toggle_rect.center(),
            1.4,
        );
        assert!(app.view.pages_footer_expanded);
        let (m_exp2, _) = capture_sidebar_text_rects_at(&ctx, &mut app, 1.5, 12, 0.025);
        click_sidebar_pos(&ctx, &mut app, m_exp2.first_tile_rect.center(), 1.9);
        assert!(!app.view.pages_footer_expanded);
        assert_eq!(app.session.doc.active_page_index(), 0);

        // Re-expand, verify invalid target rect guard and outside-touch collapse.
        let _ = capture_sidebar_text_rects_at(&ctx, &mut app, 2.0, 12, 0.025);
        click_sidebar_pos(&ctx, &mut app, m_collapsed.footer_toggle_rect.center(), 2.4);
        assert!(app.view.pages_footer_expanded);
        let _ = capture_sidebar_text_rects_at(&ctx, &mut app, 2.5, 12, 0.025);
        assert!(!pointer_pressed_outside_rect(&ctx, egui::Rect::NOTHING));

        let _ = run_timed_sidebar_frames(&ctx, &mut app, 3.0, 1, 0.025, |_| {
            vec![egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: egui::TouchId(1),
                phase: egui::TouchPhase::Start,
                pos: egui::pos2(720.0, 320.0),
                force: Some(0.5),
            }]
        });
        assert!(!pointer_pressed_outside_rect(&ctx, egui::Rect::NOTHING));
        assert!(!app.view.pages_footer_expanded);
    }

    #[test]
    fn batch_selection_footer_replaces_single_page_actions_and_respects_filter() {
        let mut app = DrawCraftApp::default();
        for _ in 0..4 {
            let _ = app.session.execute("page.add", &json!({}));
        }
        let _ = app.session.execute(
            "page.setBookmark",
            &json!({ "index": 1, "bookmarked": true }),
        );
        let _ = app.session.execute(
            "page.setBookmark",
            &json!({ "index": 3, "bookmarked": true }),
        );

        let ctx = egui::Context::default();
        let (_, rects) = capture_sidebar_text_rects(&ctx, &mut app);
        let select_r = text_rect(&rects, "Select");
        click_sidebar_pos(&ctx, &mut app, select_r.center(), 0.2);
        assert!(app.view.pages_select_mode);

        let (m_sel, sel_rects) = capture_sidebar_text_rects(&ctx, &mut app);
        assert!(!sel_rects.iter().any(|(s, _)| s == "Add Page"));
        assert!(!sel_rects.iter().any(|(s, _)| s == "Insert PDF"));

        let sel_count_r = text_rect(&sel_rects, "0 selected");
        let all_r = text_rect(&sel_rects, "All");
        let clear_r = text_rect(&sel_rects, "Clear");
        let dup_r = text_rect(&sel_rects, "Duplicate");
        let rot_r = text_rect(&sel_rects, "Rotate 90°");
        let trn_r = text_rect(&sel_rects, "Transfer");
        let export_r = text_rect(&sel_rects, "Export");
        let rem_r = text_rect(&sel_rects, "Remove Selected");

        assert!((sel_count_r.center().y - all_r.center().y).abs() <= 1.5);
        assert!((all_r.center().y - clear_r.center().y).abs() <= 1.5);
        assert!((dup_r.center().y - rot_r.center().y).abs() <= 1.5);
        assert!((trn_r.center().y - export_r.center().y).abs() <= 1.5);
        assert!(rem_r.top() > trn_r.bottom() + 8.0);
        assert!(m_sel.footer_rect.contains(rem_r.center()));

        // Filter to Saved (2) and click All: selects only the 2 visible bookmarked pages.
        let saved_r = text_rect(&sel_rects, "Saved (2)");
        click_sidebar_pos(&ctx, &mut app, saved_r.center(), 0.4);
        assert!(app.view.pages_bookmarked_only);
        click_sidebar_pos(&ctx, &mut app, all_r.center(), 0.6);
        assert_eq!(app.view.selected_page_indices, vec![1, 3]);

        let (_, two_sel_rects) = capture_sidebar_text_rects(&ctx, &mut app);
        assert!(two_sel_rects.iter().any(|(s, _)| s == "Remove 2 Pages"));

        let clear_after_r = text_rect(&two_sel_rects, "Clear");
        click_sidebar_pos(&ctx, &mut app, clear_after_r.center(), 0.8);
        assert!(app.view.selected_page_indices.is_empty());

        let done_r = text_rect(&two_sel_rects, "Done");
        click_sidebar_pos(&ctx, &mut app, done_r.center(), 1.0);
        assert!(!app.view.pages_select_mode);
        let (_, normal_rects) = capture_sidebar_text_rects(&ctx, &mut app);
        assert!(normal_rects.iter().any(|(s, _)| s == "Add Page"));
    }

    fn setup_three_page_reorder_app() -> (DrawCraftApp, egui::Context) {
        use drawcraft_doc::PaperTemplate;

        let mut app = DrawCraftApp::default();
        let _ = app
            .session
            .execute("page.add", &json!({ "template": PaperTemplate::Grid }));
        let _ = app
            .session
            .execute("page.add", &json!({ "template": PaperTemplate::Blank }));
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));
        (app, egui::Context::default())
    }

    fn page_templates(app: &DrawCraftApp) -> Vec<drawcraft_doc::PaperTemplate> {
        (0..app.session.doc.page_count())
            .filter_map(|i| app.session.doc.with_resolved_page(i, |p| p.paper_template))
            .collect()
    }

    fn run_sidebar_modal_step(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (SidebarMetrics, Vec<(String, egui::Rect)>) {
        let mut metrics = SidebarMetrics::default();
        let mut rects = Vec::new();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                metrics = show_inner(app, ui);
                modal::show(app, ui);
            },
        );
        fn collect(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let r = text.galley.rect.translate(text.pos.to_vec2());
                    out.push((text.galley.text().to_string(), r));
                }
                egui::Shape::Vec(items) => {
                    for item in items {
                        collect(item, out);
                    }
                }
                _ => {}
            }
        }
        for clipped in &out.shapes {
            collect(&clipped.shape, &mut rects);
        }
        out.drop_without_applying_deltas();
        (metrics, rects)
    }

    fn click_sidebar_modal_pos(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        pos: egui::Pos2,
        t0: f64,
    ) {
        run_sidebar_modal_step(
            ctx,
            app,
            t0,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        run_sidebar_modal_step(
            ctx,
            app,
            t0 + 0.02,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    fn settle_sidebar_modal(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        t0: f64,
    ) -> (SidebarMetrics, Vec<(String, egui::Rect)>) {
        let mut last = (SidebarMetrics::default(), Vec::new());
        for f in 1..=10 {
            last = run_sidebar_modal_step(ctx, app, t0 + f as f64 * 0.03, vec![]);
        }
        last
    }

    #[test]
    fn dragging_page_tile_requires_confirmation_modal_before_reordering() {
        use drawcraft_doc::PaperTemplate;

        let (mut app, ctx) = setup_three_page_reorder_app();
        assert_eq!(
            page_templates(&app),
            vec![
                PaperTemplate::RuledNarrow,
                PaperTemplate::Grid,
                PaperTemplate::Blank
            ]
        );

        let mut rects = Vec::new();
        for f in 0..4 {
            rects = run_sidebar_modal_step(&ctx, &mut app, f as f64 * 0.02, vec![]).1;
        }
        let p1_label = text_rect(&rects, "Page 1");
        let p3_label = text_rect(&rects, "Page 3");
        let p1_drag = egui::pos2(p1_label.center().x, p1_label.top() - 28.0);
        let p3_drop = egui::pos2(p3_label.center().x, p3_label.top() - 28.0);

        // Dropping back onto the same page tile must NOT open the confirmation modal.
        let p1_same = egui::pos2(p1_drag.x + 12.0, p1_drag.y);
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            0.20,
            vec![
                egui::Event::PointerMoved(p1_drag),
                egui::Event::PointerButton {
                    pos: p1_drag,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            0.22,
            vec![egui::Event::PointerMoved(p1_same)],
        );
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            0.24,
            vec![
                egui::Event::PointerMoved(p1_same),
                egui::Event::PointerButton {
                    pos: p1_same,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.view.pending_page_reorder, None);

        // Dragging Page 1 onto Page 3 opens ConfirmReorderPage without mutating order yet.
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            0.40,
            vec![
                egui::Event::PointerMoved(p1_drag),
                egui::Event::PointerButton {
                    pos: p1_drag,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            0.42,
            vec![egui::Event::PointerMoved(p3_drop)],
        );
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            0.44,
            vec![
                egui::Event::PointerMoved(p3_drop),
                egui::Event::PointerButton {
                    pos: p3_drop,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.view.active_modal, Some(ActiveModal::ConfirmReorderPage));
        assert_eq!(
            app.view.pending_page_reorder,
            Some(crate::PendingPageReorder {
                notebook_index: 0,
                from: 0,
                to: 2,
            })
        );
        assert_eq!(
            page_templates(&app),
            vec![
                PaperTemplate::RuledNarrow,
                PaperTemplate::Grid,
                PaperTemplate::Blank
            ]
        );

        // Cancel leaves order unchanged.
        rects = settle_sidebar_modal(&ctx, &mut app, 0.44).1;
        click_sidebar_modal_pos(&ctx, &mut app, text_rect(&rects, "Cancel").center(), 0.80);
        settle_sidebar_modal(&ctx, &mut app, 0.82);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.view.pending_page_reorder, None);
        assert_eq!(
            page_templates(&app),
            vec![
                PaperTemplate::RuledNarrow,
                PaperTemplate::Grid,
                PaperTemplate::Blank
            ]
        );

        // Drag again and confirm with "Move Page" -> reorders pages cleanly.
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            1.20,
            vec![
                egui::Event::PointerMoved(p1_drag),
                egui::Event::PointerButton {
                    pos: p1_drag,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            1.22,
            vec![egui::Event::PointerMoved(p3_drop)],
        );
        run_sidebar_modal_step(
            &ctx,
            &mut app,
            1.24,
            vec![
                egui::Event::PointerMoved(p3_drop),
                egui::Event::PointerButton {
                    pos: p3_drop,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.view.active_modal, Some(ActiveModal::ConfirmReorderPage));
        rects = settle_sidebar_modal(&ctx, &mut app, 1.24).1;
        click_sidebar_modal_pos(
            &ctx,
            &mut app,
            text_rect(&rects, "Move Page").center(),
            1.60,
        );
        settle_sidebar_modal(&ctx, &mut app, 1.62);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.view.pending_page_reorder, None);
        assert_eq!(
            page_templates(&app),
            vec![
                PaperTemplate::Grid,
                PaperTemplate::Blank,
                PaperTemplate::RuledNarrow
            ]
        );
    }

    #[test]
    fn clicking_page_reorder_arrows_requires_confirmation_modal_before_reordering() {
        use drawcraft_doc::PaperTemplate;

        let (mut app, ctx) = setup_three_page_reorder_app();
        let mut m_arrow = SidebarMetrics::default();
        for f in 0..4 {
            m_arrow = run_sidebar_modal_step(&ctx, &mut app, f as f64 * 0.02, vec![]).0;
        }
        let move_down_pt = egui::pos2(
            m_arrow.first_bookmark_rect.left() - 9.5,
            m_arrow.first_bookmark_rect.center().y,
        );
        click_sidebar_modal_pos(&ctx, &mut app, move_down_pt, 0.20);
        assert_eq!(app.view.active_modal, Some(ActiveModal::ConfirmReorderPage));
        assert_eq!(
            app.view.pending_page_reorder,
            Some(crate::PendingPageReorder {
                notebook_index: 0,
                from: 0,
                to: 1,
            })
        );
        assert_eq!(
            page_templates(&app),
            vec![
                PaperTemplate::RuledNarrow,
                PaperTemplate::Grid,
                PaperTemplate::Blank
            ]
        );

        // Cancel keeps page order unchanged.
        let rects = settle_sidebar_modal(&ctx, &mut app, 0.22).1;
        click_sidebar_modal_pos(&ctx, &mut app, text_rect(&rects, "Cancel").center(), 0.60);
        settle_sidebar_modal(&ctx, &mut app, 0.62);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(
            page_templates(&app),
            vec![
                PaperTemplate::RuledNarrow,
                PaperTemplate::Grid,
                PaperTemplate::Blank
            ]
        );

        // Click MoveDown again and confirm via "Move Page".
        click_sidebar_modal_pos(&ctx, &mut app, move_down_pt, 1.00);
        assert_eq!(app.view.active_modal, Some(ActiveModal::ConfirmReorderPage));
        let rects_confirm = settle_sidebar_modal(&ctx, &mut app, 1.02).1;
        click_sidebar_modal_pos(
            &ctx,
            &mut app,
            text_rect(&rects_confirm, "Move Page").center(),
            1.40,
        );
        settle_sidebar_modal(&ctx, &mut app, 1.42);
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.view.pending_page_reorder, None);
        assert_eq!(
            page_templates(&app),
            vec![
                PaperTemplate::Grid,
                PaperTemplate::RuledNarrow,
                PaperTemplate::Blank
            ]
        );
    }

    fn count_line_shapes_in_rect(shape: &egui::Shape, region: egui::Rect) -> usize {
        match shape {
            egui::Shape::Path(path) => {
                usize::from(path.points.iter().any(|pt| region.contains(*pt)))
            }
            egui::Shape::LineSegment { points, .. } => {
                usize::from(points.iter().any(|pt| region.contains(*pt)))
            }
            egui::Shape::Vec(items) => items
                .iter()
                .map(|s| count_line_shapes_in_rect(s, region))
                .sum(),
            _ => 0,
        }
    }

    fn run_sidebar_drag_capture(
        ctx: &egui::Context,
        app: &mut DrawCraftApp,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Rect)>, Vec<egui::epaint::ClippedShape>) {
        let mut rects = Vec::new();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                crate::theme::apply_visuals(ui.ctx(), app.session.prefs.theme);
                let _ = show_inner(app, ui);
                modal::show(app, ui);
            },
        );
        fn collect(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let r = text.galley.rect.translate(text.pos.to_vec2());
                    out.push((text.galley.text().to_string(), r));
                }
                egui::Shape::Vec(items) => {
                    for item in items {
                        collect(item, out);
                    }
                }
                _ => {}
            }
        }
        for clipped in &out.shapes {
            collect(&clipped.shape, &mut rects);
        }
        let shapes = out.shapes.clone();
        out.drop_without_applying_deltas();
        (rects, shapes)
    }

    #[test]
    fn dragging_page_renders_floating_page_preview_that_follows_pointer() {
        let (mut app, ctx) = setup_three_page_reorder_app();
        // Add a visible stroke to Page 1 so the floating preview renders actual page ink.
        app.session
            .execute(
                "stroke.draw",
                &json!({
                    "points": [
                        { "x": 120.0, "y": 240.0, "pressure": 0.8 },
                        { "x": 980.0, "y": 1160.0, "pressure": 0.8 }
                    ]
                }),
            )
            .expect("draw stroke on Page 1");

        let mut rects = Vec::new();
        for f in 0..4 {
            rects = run_sidebar_drag_capture(&ctx, &mut app, f as f64 * 0.02, vec![]).0;
        }
        let p1_label = text_rect(&rects, "Page 1");
        let p3_label = text_rect(&rects, "Page 3");
        let p1_drag = egui::pos2(p1_label.center().x, p1_label.top() - 28.0);
        let p3_drop = egui::pos2(p3_label.center().x, p3_label.top() - 28.0);

        // Press and move slightly on Page 1 to start dragging.
        run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.20,
            vec![
                egui::Event::PointerMoved(p1_drag),
                egui::Event::PointerButton {
                    pos: p1_drag,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let drag_pos_1 = egui::pos2(p1_drag.x + 12.0, p1_drag.y + 4.0);
        let (rects_1, shapes_1) = run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.22,
            vec![egui::Event::PointerMoved(drag_pos_1)],
        );
        assert_eq!(app.view.pages_drag_source, Some(0));
        let drop_hint_1 = text_rect(&rects_1, "Drop on page");
        assert!(
            drop_hint_1.left() >= drag_pos_1.x + 10.0 && drop_hint_1.top() > drag_pos_1.y + 90.0,
            "floating ghost caption {drop_hint_1:?} should follow near pointer {drag_pos_1:?}"
        );
        // Floating sheet above the caption contains painted lines (ruling + stroke).
        let ghost_sheet_zone = egui::Rect::from_min_max(
            egui::pos2(drag_pos_1.x + 10.0, drag_pos_1.y + 10.0),
            egui::pos2(drop_hint_1.right() + 40.0, drop_hint_1.top()),
        );
        let line_count: usize = shapes_1
            .iter()
            .map(|c| count_line_shapes_in_rect(&c.shape, ghost_sheet_zone))
            .sum();
        assert!(
            line_count > 0,
            "expected miniature paper/stroke lines inside floating page area {ghost_sheet_zone:?}"
        );

        // Drag onto open canvas area outside sidebar: floating preview moves along with pointer.
        let drag_pos_2 = egui::pos2(420.0, 260.0);
        let (rects_2, _) = run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.24,
            vec![egui::Event::PointerMoved(drag_pos_2)],
        );
        let drop_hint_2 = text_rect(&rects_2, "Drop on page");
        assert!(
            (drop_hint_2.left() - (drag_pos_2.x + 20.0)).abs() < 16.0,
            "floating preview {drop_hint_2:?} did not track pointer to {drag_pos_2:?}"
        );
        let floating_p1 = rects_2
            .iter()
            .filter(|(label, _)| label == "Page 1")
            .map(|(_, rect)| *rect)
            .find(|rect| rect.left() > DEFAULT_SIDEBAR_WIDTH)
            .expect("floating Page 1 badge outside sidebar");
        assert!(floating_p1.left() > drag_pos_2.x);

        // Hover over Page 3 while still dragging (dark theme also renders cleanly): caption shows destination.
        app.session.prefs.theme = drawcraft_doc::UiTheme::Dark;
        let (rects_3, _) = run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.26,
            vec![egui::Event::PointerMoved(p3_drop)],
        );
        let move_hint = text_rect(&rects_3, "Move to Pos 3");
        assert!(move_hint.left() >= p3_drop.x + 10.0);
        assert!(!rects_3.iter().any(|(s, _)| s == "Drop on page"));

        // Release on Page 3 opens confirmation modal and clears floating preview.
        let (rects_done, _) = run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.28,
            vec![
                egui::Event::PointerMoved(p3_drop),
                egui::Event::PointerButton {
                    pos: p3_drop,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.view.pages_drag_source, None);
        assert_eq!(app.view.active_modal, Some(ActiveModal::ConfirmReorderPage));
        assert!(!rects_done.iter().any(|(s, _)| s == "Move to Pos 3"));
        assert!(!rects_done.iter().any(|(s, _)| s == "Drop on page"));
    }

    #[test]
    fn pressing_escape_cancels_page_drag_and_hides_floating_preview() {
        let (mut app, ctx) = setup_three_page_reorder_app();
        let mut rects = Vec::new();
        for f in 0..4 {
            rects = run_sidebar_drag_capture(&ctx, &mut app, f as f64 * 0.02, vec![]).0;
        }
        let p1_label = text_rect(&rects, "Page 1");
        let p3_label = text_rect(&rects, "Page 3");
        let p1_drag = egui::pos2(p1_label.center().x, p1_label.top() - 28.0);
        let p3_drop = egui::pos2(p3_label.center().x, p3_label.top() - 28.0);

        run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.20,
            vec![
                egui::Event::PointerMoved(p1_drag),
                egui::Event::PointerButton {
                    pos: p1_drag,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let (rects_dragging, _) = run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.22,
            vec![egui::Event::PointerMoved(egui::pos2(
                p1_drag.x + 12.0,
                p1_drag.y,
            ))],
        );
        assert_eq!(app.view.pages_drag_source, Some(0));
        assert!(rects_dragging.iter().any(|(s, _)| s == "Drop on page"));

        // Escape aborts drag immediately.
        let (rects_esc, _) = run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.24,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.view.pages_drag_source, None);
        assert!(!rects_esc.iter().any(|(s, _)| s == "Drop on page"));

        // Moving to Page 3 and releasing after Escape does NOT trigger reorder.
        run_sidebar_drag_capture(
            &ctx,
            &mut app,
            0.26,
            vec![
                egui::Event::PointerMoved(p3_drop),
                egui::Event::PointerButton {
                    pos: p3_drop,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(app.view.active_modal, None);
        assert_eq!(app.view.pending_page_reorder, None);
    }

    #[test]
    fn clamp_ghost_pos_keeps_floating_preview_within_screen_and_handles_tiny_bounds() {
        let screen = egui::Rect::from_min_max(egui::pos2(8.0, 8.0), egui::pos2(1272.0, 792.0));
        let size = egui::vec2(100.0, 160.0);

        let inside = clamp_ghost_pos(egui::pos2(200.0, 300.0), size, screen);
        assert_eq!(inside, egui::pos2(200.0, 300.0));

        let clipped = clamp_ghost_pos(egui::pos2(1240.0, 760.0), size, screen);
        assert_eq!(clipped, egui::pos2(1172.0, 632.0));

        // Tiny / inverted screen rect never panics.
        let tiny = egui::Rect::from_min_size(egui::pos2(8.0, 8.0), egui::vec2(20.0, 20.0));
        let clamped_tiny = clamp_ghost_pos(egui::pos2(50.0, 50.0), size, tiny);
        assert_eq!(clamped_tiny, egui::pos2(8.0, 8.0));

        let empty = egui::Rect::NOTHING;
        let fallback = clamp_ghost_pos(egui::pos2(40.0, 50.0), size, empty);
        assert_eq!(fallback, egui::pos2(40.0, 50.0));
    }

    #[test]
    fn thumbnail_decimation_reduces_dense_points_and_highlighter_avoids_joint_overdraw() {
        let mut pts = Vec::with_capacity(400);
        for i in 0..400 {
            let mut s = drawcraft_doc::StrokeSample::from_xy_pressure(
                100.0 + (i as f64) * 2.0,
                300.0 + ((i as f64) * 0.2).sin() * 20.0,
                0.7,
            );
            s.time_ms = i as f64;
            pts.push(s);
        }
        let mut doc = drawcraft_doc::Document::default();
        let hl_pen = drawcraft_doc::PenSettings {
            kind: PenKind::Highlighter,
            size: 18.0,
            color: drawcraft_color::Color::from_rgba8(250, 210, 40, 255),
            ..drawcraft_doc::PenSettings::default()
        };
        let bounds = drawcraft_doc::compute_stroke_bounds(&pts, hl_pen.size);
        let pts_arc = Arc::new(pts);
        doc.strokes.push(Arc::new(InkStroke {
            id: drawcraft_doc::StrokeId(1),
            pen: hl_pen,
            raw_points: Arc::clone(&pts_arc),
            smoothed_points: pts_arc,
            bounds,
        }));
        let sheet_size = egui::vec2(100.0, 140.0);
        doc.with_resolved_page(0, |page| {
            let built =
                build_thumbnail_strokes(sheet_size, page, drawcraft_doc::UiTheme::Light, true);
            assert_eq!(built.len(), 1);
            assert!(built[0].pts.len() < 120);
            assert!(built[0].is_highlighter);
            let max_expected_alpha = built[0].color.a();
            let img = rasterize_thumbnail_strokes_image(sheet_size, &built);
            assert_eq!(img.size, [200, 280]);
            let max_px_alpha = img.pixels.iter().map(|c| c.a()).max().unwrap_or(0);
            assert!(max_px_alpha > 0);
            assert!(max_px_alpha <= max_expected_alpha);
        });
    }

    #[test]
    fn thumbnail_rasterizer_draws_shapes_and_micro_text_into_texture_image() {
        let mut doc = drawcraft_doc::Document::default();
        doc.shapes.push(Arc::new(drawcraft_doc::GeometricShape {
            kind: drawcraft_doc::ShapeKind::Polygon,
            x1: 120.0,
            y1: 160.0,
            x2: 420.0,
            y2: 460.0,
            angle_degrees: 0.0,
            curve: 12.0,
            color: drawcraft_color::Color::from_rgba8(200, 40, 40, 255),
            width: 3.0,
            fill_color: Some(drawcraft_color::Color::from_rgba8(240, 120, 120, 128)),
            line_style: drawcraft_doc::ShapeLineStyle::Dashed,
            points: vec![
                drawcraft_geom::Point::new(120.0, 160.0),
                drawcraft_geom::Point::new(420.0, 160.0),
                drawcraft_geom::Point::new(420.0, 460.0),
                drawcraft_geom::Point::new(120.0, 460.0),
            ],
        }));
        doc.texts.push(Arc::new(drawcraft_doc::TextItem {
            text: "Small dense note body that renders directly into thumbnail raster".into(),
            x: 80.0,
            y: 80.0,
            size: 12.0,
            color: drawcraft_color::Color::BLACK,
            font: drawcraft_doc::TextFont::Sans,
            line_spacing: 1.2,
            letter_spacing: 0.0,
            angle_degrees: 0.0,
            max_width: Some(300.0),
            alignment: drawcraft_doc::TextAlignment::Left,
            spans: Vec::new(),
            link_url: None,
        }));
        let ctx = egui::Context::default();
        let mut img_opt = None;
        let mut overlay_count = 0usize;
        let raw = egui::RawInput::default();
        let out = ctx.run_ui(raw, |ui| {
            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 140.0));
            doc.with_resolved_page(0, |page| {
                let (img, shapes) = build_thumbnail_foreground_assets(
                    ui.painter(),
                    rect,
                    page,
                    drawcraft_doc::UiTheme::Light,
                    true,
                );
                img_opt = img;
                overlay_count = shapes.len();
            });
        });
        out.drop_without_applying_deltas();
        assert_eq!(overlay_count, 0);
        assert!(
            img_opt.is_some(),
            "expected rasterized foreground thumbnail image"
        );
        if let Some(img) = img_opt {
            assert!(img.pixels.iter().any(|p| p.a() > 0));
        }
    }

    #[test]
    fn shift_click_preserves_selection_sequence_and_right_click_export_opens_export_modal() {
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.add", &json!({}));
        let _ = app.session.execute("page.select", &json!({ "index": 0 }));
        let ctx = egui::Context::default();

        let send_sidebar_btn = |ctx: &egui::Context,
                                app: &mut DrawCraftApp,
                                pos: egui::Pos2,
                                time: f64,
                                button: egui::PointerButton,
                                modifiers: egui::Modifiers| {
            for pressed in [true, false] {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    time: Some(time + if pressed { 0.0 } else { 0.03 }),
                    events: vec![
                        egui::Event::ModifiersChanged(modifiers),
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button,
                            pressed,
                            modifiers,
                        },
                    ],
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    let _ = show_inner(app, ui);
                });
                out.drop_without_applying_deltas();
            }
        };

        let (m0, rects) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.0, 4, 0.025);
        let tile_label_center = |label: &str| {
            rects
                .iter()
                .find(|(text, rect)| text == label && m0.scroll_rect.contains(rect.center()))
                .map(|(_, rect)| rect.center())
                .unwrap_or_else(|| panic!("expected tile label {label:?} in scroll_rect"))
        };
        let p3_pos = tile_label_center("Page 3");
        let p1_pos = tile_label_center("Page 1");
        let p2_pos = tile_label_center("Page 2");

        // Shift+Click Page 3 first, then Page 1 -> sequence must remain [2, 0] (unsorted).
        send_sidebar_btn(
            &ctx,
            &mut app,
            p3_pos,
            0.2,
            egui::PointerButton::Primary,
            egui::Modifiers::SHIFT,
        );
        send_sidebar_btn(
            &ctx,
            &mut app,
            p1_pos,
            0.35,
            egui::PointerButton::Primary,
            egui::Modifiers::SHIFT,
        );
        assert!(app.view.pages_select_mode);
        assert_eq!(app.view.selected_page_indices, vec![2, 0]);

        // Right-click any page tile (Page 2) to open context menu and click "Export".
        send_sidebar_btn(
            &ctx,
            &mut app,
            p2_pos,
            0.5,
            egui::PointerButton::Secondary,
            egui::Modifiers::NONE,
        );
        let (_, export_center, _) = find_sidebar_label_center(&ctx, &mut app, "Export");
        let export_pos = export_center.expect("context menu Export button should appear");
        send_sidebar_btn(
            &ctx,
            &mut app,
            export_pos,
            0.7,
            egui::PointerButton::Primary,
            egui::Modifiers::NONE,
        );
        assert_eq!(app.view.active_modal, Some(ActiveModal::Export));
        assert_eq!(app.view.export_target_pages, Some(vec![2, 0]));

        // Exit modal and selection mode, settle frames, then right-click single Page 2 -> targets only Page 2.
        crate::modal::close(&mut app);
        app.view.pages_select_mode = false;
        app.view.selected_page_indices.clear();
        let (m_reset, rects_reset) = capture_sidebar_text_rects_at(&ctx, &mut app, 0.8, 2, 0.025);
        let p2_pos_single = rects_reset
            .iter()
            .find(|(text, rect)| text == "Page 2" && m_reset.scroll_rect.contains(rect.center()))
            .map(|(_, rect)| rect.center())
            .expect("tile Page 2 label after clearing selection");
        send_sidebar_btn(
            &ctx,
            &mut app,
            p2_pos_single,
            0.95,
            egui::PointerButton::Secondary,
            egui::Modifiers::NONE,
        );
        let (_, single_export_center, _) = find_sidebar_label_center(&ctx, &mut app, "Export");
        let single_pos = single_export_center.expect("context menu Export button for single page");
        send_sidebar_btn(
            &ctx,
            &mut app,
            single_pos,
            1.1,
            egui::PointerButton::Primary,
            egui::Modifiers::NONE,
        );
        assert_eq!(app.view.active_modal, Some(ActiveModal::Export));
        assert_eq!(app.view.export_target_pages, Some(vec![1]));

        // Ctrl+Click does not enter multi-page selection; it switches the active page normally.
        crate::modal::close(&mut app);
        let (m_ctrl, rects_ctrl) = capture_sidebar_text_rects_at(&ctx, &mut app, 1.2, 2, 0.025);
        let p2_pos_ctrl = rects_ctrl
            .iter()
            .find(|(text, rect)| text == "Page 2" && m_ctrl.scroll_rect.contains(rect.center()))
            .map(|(_, rect)| rect.center())
            .expect("tile Page 2 label after closing export modal");
        send_sidebar_btn(
            &ctx,
            &mut app,
            p2_pos_ctrl,
            1.3,
            egui::PointerButton::Primary,
            egui::Modifiers::CTRL,
        );
        assert!(!app.view.pages_select_mode);
        assert!(app.view.selected_page_indices.is_empty());
        assert_eq!(app.session.doc.active_page_index(), 1);
    }

    #[test]
    fn thumbnail_highlighter_with_offpage_left_top_segment_completes_without_hanging() {
        let mut doc = drawcraft_doc::Document::new(800, 1000);
        let hl_pen = drawcraft_doc::PenSettings {
            kind: PenKind::Highlighter,
            size: 18.0,
            color: drawcraft_color::Color::from_rgba8(250, 210, 40, 255),
            ..drawcraft_doc::PenSettings::default()
        };

        let left_pts = vec![
            drawcraft_doc::StrokeSample::from_xy_pressure(-80.0, 220.0, 0.7),
            drawcraft_doc::StrokeSample::from_xy_pressure(-40.0, 220.0, 0.7),
            drawcraft_doc::StrokeSample::from_xy_pressure(180.0, 220.0, 0.7),
        ];
        let left_arc = Arc::new(left_pts);
        let left_bounds = drawcraft_doc::compute_stroke_bounds(&left_arc, hl_pen.size);
        doc.strokes.push(Arc::new(InkStroke {
            id: drawcraft_doc::StrokeId(1),
            pen: hl_pen.clone(),
            raw_points: Arc::clone(&left_arc),
            smoothed_points: left_arc,
            bounds: left_bounds,
        }));

        let top_pts = vec![
            drawcraft_doc::StrokeSample::from_xy_pressure(240.0, -80.0, 0.7),
            drawcraft_doc::StrokeSample::from_xy_pressure(240.0, -40.0, 0.7),
            drawcraft_doc::StrokeSample::from_xy_pressure(240.0, 180.0, 0.7),
        ];
        let top_arc = Arc::new(top_pts);
        let top_bounds = drawcraft_doc::compute_stroke_bounds(&top_arc, hl_pen.size);
        doc.strokes.push(Arc::new(InkStroke {
            id: drawcraft_doc::StrokeId(2),
            pen: hl_pen,
            raw_points: Arc::clone(&top_arc),
            smoothed_points: top_arc,
            bounds: top_bounds,
        }));

        let sheet_size = egui::vec2(100.0, 125.0);
        let built = doc
            .with_resolved_page(0, |page| {
                build_thumbnail_strokes(sheet_size, page, drawcraft_doc::UiTheme::Light, true)
            })
            .expect("page 0 should exist");
        assert_eq!(built.len(), 2);

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let img = rasterize_thumbnail_strokes_image(sheet_size, &built);
            let _ = tx.send(img);
        });

        let img = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("thumbnail stroke rasterizer froze on off-page segment");
        assert!(img.pixels.iter().any(|px| px.a() > 0));
    }

    #[test]
    fn thumbnail_offpage_all_directions_and_reopen_home_render_cleanly() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut doc = drawcraft_doc::Document::new(800, 1000);
        let hl_pen = drawcraft_doc::PenSettings {
            kind: PenKind::Highlighter,
            size: 18.0,
            color: drawcraft_color::Color::from_rgba8(250, 210, 40, 255),
            ..drawcraft_doc::PenSettings::default()
        };
        let ink_pen = drawcraft_doc::PenSettings::fountain_pen();

        let paths: Vec<(drawcraft_doc::PenSettings, Vec<(f64, f64)>)> = vec![
            (
                hl_pen.clone(),
                vec![(-80.0, 200.0), (-30.0, 200.0), (140.0, 200.0)],
            ),
            (
                hl_pen.clone(),
                vec![(300.0, -80.0), (300.0, -30.0), (300.0, 140.0)],
            ),
            (
                hl_pen.clone(),
                vec![(860.0, 300.0), (920.0, 300.0), (700.0, 300.0)],
            ),
            (
                hl_pen.clone(),
                vec![(300.0, 1060.0), (300.0, 1120.0), (300.0, 900.0)],
            ),
            (hl_pen.clone(), vec![(-500.0, -500.0), (-200.0, -200.0)]),
            (hl_pen, vec![(-1e9, 200.0), (1e9, 200.0)]),
            (
                ink_pen,
                vec![(-1e9, -1e9), (-100.0, -100.0), (200.0, 200.0)],
            ),
        ];
        for (idx, (pen, coords)) in paths.into_iter().enumerate() {
            let pts: Vec<_> = coords
                .into_iter()
                .map(|(x, y)| drawcraft_doc::StrokeSample::from_xy_pressure(x, y, 0.7))
                .collect();
            let arc = Arc::new(pts);
            let bounds = drawcraft_doc::compute_stroke_bounds(&arc, pen.size);
            doc.strokes.push(Arc::new(InkStroke {
                id: drawcraft_doc::StrokeId(idx as u64 + 1),
                pen,
                raw_points: Arc::clone(&arc),
                smoothed_points: arc,
                bounds,
            }));
        }
        doc.shapes.push(Arc::new(drawcraft_doc::GeometricShape {
            kind: drawcraft_doc::ShapeKind::Polygon,
            x1: -200.0,
            y1: -200.0,
            x2: 120.0,
            y2: 120.0,
            angle_degrees: 0.0,
            curve: 0.0,
            color: drawcraft_color::Color::from_rgba8(200, 40, 40, 255),
            width: 3.0,
            fill_color: Some(drawcraft_color::Color::from_rgba8(240, 120, 120, 128)),
            line_style: drawcraft_doc::ShapeLineStyle::Solid,
            points: vec![
                drawcraft_geom::Point::new(-200.0, -200.0),
                drawcraft_geom::Point::new(120.0, -200.0),
                drawcraft_geom::Point::new(120.0, 120.0),
                drawcraft_geom::Point::new(-200.0, 120.0),
            ],
        }));
        doc.texts.push(Arc::new(drawcraft_doc::TextItem {
            text: "Offpage start heading".into(),
            x: -120.0,
            y: -40.0,
            size: 12.0,
            color: drawcraft_color::Color::BLACK,
            font: drawcraft_doc::TextFont::Sans,
            line_spacing: 1.2,
            letter_spacing: 0.0,
            angle_degrees: 0.0,
            max_width: Some(300.0),
            alignment: drawcraft_doc::TextAlignment::Left,
            spans: Vec::new(),
            link_url: None,
        }));
        doc.sync_active_to_pages();

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("drawcraft_thumb_reopen_{nanos}"));
        let _ = std::fs::remove_dir_all(&dir);
        struct TempDirGuard(std::path::PathBuf);
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = TempDirGuard(dir.clone());

        {
            let mut session = drawcraft_engine::Session::open_desktop_library(dir.clone());
            session
                .execute("notebook.create", &json!({ "title": "Offpage Note" }))
                .unwrap();
            let pen = doc.strokes[0].pen.clone();
            let raw = doc.strokes[0].raw_points.as_ref().clone();
            assert!(session.commit_raw_stroke(pen, raw).is_some());
            session.save_library().unwrap();
        }

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut app = DrawCraftApp::new(
                drawcraft_engine::Session::open_desktop_library(dir),
                crate::PlatformHooks::default(),
            );
            let ctx = egui::Context::default();
            for t in [0.02, 0.05] {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    time: Some(t),
                    predicted_dt: 0.016,
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| crate::home::show(&mut app, ui));
                out.drop_without_applying_deltas();
            }

            let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 125.0));
            let fg_ctx = egui::Context::default();
            let mut has_fg = false;
            let out2 = fg_ctx.run_ui(egui::RawInput::default(), |ui| {
                doc.with_resolved_page(0, |page| {
                    let (img, _) = build_thumbnail_foreground_assets(
                        ui.painter(),
                        rect,
                        page,
                        drawcraft_doc::UiTheme::Light,
                        true,
                    );
                    has_fg = img.is_some_and(|im| im.pixels.iter().any(|p| p.a() > 0));
                });
            });
            out2.drop_without_applying_deltas();
            let _ = tx.send(has_fg);
        });

        let has_fg = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("reopening notebook with off-page highlighter stroke froze Home render");
        assert!(has_fg);
    }
}
