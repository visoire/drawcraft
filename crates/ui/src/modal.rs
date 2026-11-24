//! Standard 3-part modal dialog architecture for DrawCraft.
//!
//! Every dialog uses the same structural contract:
//! 1. **Header**: semantic icon badge + title/subtitle + top-right aligned `X` close button.
//! 2. **Body**: adaptive content area that scrolls cleanly only when viewport space is tight.
//! 3. **Footer**: sticky bottom action bar (`Cancel` + primary CTA, optional left action) separated
//!    by a full-width divider so actions are always reachable and never occluded by scrollbars.

use egui::Color32;

use crate::icons::{Icon, icon_button, paint_icon};
use crate::theme::Tokens;
use crate::{DrawCraftApp, customization_modals, export_modal, folder_modals};

/// Identifies which focused dialog is currently open over the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveModal {
    Export,
    NewNotebook,
    NotebookSettings,
    ConfirmDeleteNotebook,
    ConfirmReorderPage,
    AccountSync,
    FirstAccessNickname,
    SyncConflict,
    TransferPages,
    NewFolder,
    RenameFolder,
    DeleteFolder,
    FolderColor,
    TagColor,
}

struct DialogSpec {
    title: &'static str,
    width: f32,
    /// Minimum body height locked across stepped flows so switching steps never resizes the modal.
    min_body_height: f32,
}

#[derive(Clone, Copy)]
struct ModalStepSpec {
    label: &'static str,
    icon: Icon,
}

const NOTEBOOK_SECTION_STEPS: &[ModalStepSpec] = &[
    ModalStepSpec {
        label: "Details & Cover",
        icon: Icon::Notebook,
    },
    ModalStepSpec {
        label: "Size & Orientation",
        icon: Icon::FitPage,
    },
    ModalStepSpec {
        label: "Paper & Templates",
        icon: Icon::Paper,
    },
];

impl ActiveModal {
    const fn id_tag(self) -> u8 {
        match self {
            Self::Export => 1,
            Self::NewNotebook => 2,
            Self::NotebookSettings => 3,
            Self::ConfirmDeleteNotebook => 4,
            Self::AccountSync => 5,
            Self::FirstAccessNickname => 14,
            Self::SyncConflict => 15,
            Self::TransferPages => 6,
            Self::NewFolder => 7,
            Self::RenameFolder => 8,
            Self::DeleteFolder => 9,
            Self::ConfirmReorderPage => 11,
            Self::FolderColor => 12,
            Self::TagColor => 13,
        }
    }

    const fn spec(self) -> DialogSpec {
        match self {
            Self::Export => DialogSpec {
                title: "Export Notebook",
                width: 420.0,
                min_body_height: 0.0,
            },
            Self::NewNotebook => DialogSpec {
                title: "New Notebook",
                width: 640.0,
                min_body_height: 440.0,
            },
            Self::NotebookSettings => DialogSpec {
                title: "Notebook Settings",
                width: 680.0,
                min_body_height: 512.0,
            },
            Self::ConfirmDeleteNotebook => DialogSpec {
                title: "Move Notebook to Trash?",
                width: 440.0,
                min_body_height: 0.0,
            },
            Self::ConfirmReorderPage => DialogSpec {
                title: "Move Page Position?",
                width: 440.0,
                min_body_height: 0.0,
            },
            Self::AccountSync => DialogSpec {
                title: "Account & Cloud Sync",
                width: 530.0,
                min_body_height: 0.0,
            },
            Self::TransferPages => DialogSpec {
                title: "Transfer or Merge Pages",
                width: 520.0,
                min_body_height: 0.0,
            },
            Self::NewFolder => DialogSpec {
                title: "New Folder",
                width: 440.0,
                min_body_height: 0.0,
            },
            Self::RenameFolder => DialogSpec {
                title: "Edit Folder",
                width: 440.0,
                min_body_height: 0.0,
            },
            Self::DeleteFolder => DialogSpec {
                title: "Delete Folder?",
                width: 440.0,
                min_body_height: 0.0,
            },
            Self::FolderColor => DialogSpec {
                title: "Folder Color",
                width: 440.0,
                min_body_height: 0.0,
            },
            Self::TagColor => DialogSpec {
                title: "Tag Color",
                width: 440.0,
                min_body_height: 0.0,
            },
            Self::FirstAccessNickname => DialogSpec {
                title: "Welcome to DrawCraft",
                width: 420.0,
                min_body_height: 0.0,
            },
            Self::SyncConflict => DialogSpec {
                title: "Sync Conflict Detected",
                width: 480.0,
                min_body_height: 0.0,
            },
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
struct ModalMetrics {
    modal_rect: egui::Rect,
    close_btn_rect: egui::Rect,
    body_rect: egui::Rect,
    footer_rect: egui::Rect,
    inner_margin: f32,
}

/// Opens `target`, initializing its draft fields cleanly from active session state.
pub(crate) fn open(app: &mut DrawCraftApp, target: ActiveModal) {
    app.view.modal_open_seq = app.view.modal_open_seq.wrapping_add(1);
    crate::canvas::commit_pending_text(app);
    app.view.shape_insert_mode = false;
    app.view.shape_drag_start = None;
    app.view.shape_drag_end = None;
    app.view.shape_drag_touch_id = None;
    app.view.ruler_active = false;
    app.view.ruler_drag_mode = None;
    app.view.ruler_drag_touch_id = None;
    app.view.ruler_drag_anchor = None;
    app.view.text_insert_mode = false;
    app.view.text_style_open = false;
    if target != ActiveModal::ConfirmDeleteNotebook {
        app.view.pending_delete_notebook = None;
    }
    if target != ActiveModal::ConfirmReorderPage {
        app.view.pending_page_reorder = None;
    }
    app.view.tag_edit_index = None;
    app.view.tag_edit_draft.clear();
    app.view.tag_edit_focus_pending = false;
    match target {
        ActiveModal::Export => {
            if app.view.export_path_stem.trim().is_empty() {
                app.view.export_path_stem =
                    crate::inspector::title_to_export_stem(&app.session.doc.title);
            }
            if app.view.export_target_pages.is_none()
                && app.view.pages_select_mode
                && !app.view.selected_page_indices.is_empty()
            {
                app.view.export_target_pages = Some(app.view.selected_page_indices.clone());
            }
            app.view.export_feedback = None;
        }
        ActiveModal::NewNotebook => {
            app.view.new_notebook_step = 0;
            app.view.new_notebook_preview_tab = 0;
            sync_new_notebook_from_preset(app, app.view.new_notebook_preset);
            app.view.new_notebook_folder = match crate::library_folders::current_scope(app) {
                crate::library_folders::LibraryScope::Folder(path) => path,
                _ => String::new(),
            };
            app.view.new_notebook_favorite = false;
            app.view.new_notebook_tags.clear();
            // Notebooks started from Favorites or a tag view land back in that same view.
            match crate::library_folders::current_scope(app) {
                crate::library_folders::LibraryScope::Favorites => {
                    app.view.new_notebook_favorite = true;
                }
                crate::library_folders::LibraryScope::Tag(tag) => {
                    app.view.new_notebook_tags.push(tag);
                }
                _ => {}
            }
            app.view.new_notebook_tag_input.clear();
            app.view.new_notebook_custom_template_id = None;
            app.view.template_import_error = None;
            app.view.folder_picker_new_open = false;
            app.view.folder_picker_new_name.clear();
        }
        ActiveModal::NotebookSettings => {
            app.view.notebook_settings_step = 0;
            app.view.notebook_title_draft = app.session.doc.title.clone();
            app.view.notebook_cover_draft = app.session.doc.cover;
            app.view.notebook_folder_draft = app.session.doc.folder_path.clone();
            app.view.notebook_favorite_draft = app.session.doc.favorite;
            app.view.notebook_tags_draft = app.session.doc.tags.clone();
            app.view.notebook_tag_input_draft.clear();
            app.view.folder_picker_new_open = false;
            app.view.folder_picker_new_name.clear();
            app.view
                .cover_color_draft
                .sync_from_cover(app.session.doc.cover);
            app.view.page_setup_format =
                crate::page_format_picker::PageFormatDraft::from_dimensions(
                    app.session.doc.width,
                    app.session.doc.height,
                );
            app.view.page_setup_template = app.session.doc.paper_template;
            app.view.page_setup_color = app.session.doc.paper_color;
            app.view.page_setup_custom_template_id = app.session.doc.custom_template_id.clone();
            app.view.page_setup_clear_custom = false;
            app.view.page_setup_quarter_turns = 0;
            app.view.page_setup_apply_all = false;
            app.view.template_import_error = None;
        }
        ActiveModal::ConfirmDeleteNotebook
        | ActiveModal::ConfirmReorderPage
        | ActiveModal::NewFolder
        | ActiveModal::RenameFolder
        | ActiveModal::DeleteFolder
        | ActiveModal::FolderColor
        | ActiveModal::TagColor
        | ActiveModal::SyncConflict => {}
        ActiveModal::FirstAccessNickname => {
            app.view.first_access_error = None;
            app.view.first_access_focus_pending = true;
        }
        ActiveModal::AccountSync => {
            crate::account_modal::sync_draft_from_session(app);
        }
        ActiveModal::TransferPages => {
            if app.view.transfer_pages_list.is_empty()
                && app.view.transfer_merge_whole_notebook.is_none()
            {
                app.view.transfer_pages_list = vec![app.session.doc.active_page_index()];
            }
            let cur = app.session.active_notebook;
            let alt = (0..app.session.notebooks.len())
                .find(|&idx| idx != cur)
                .unwrap_or(0);
            if app.view.transfer_target_notebook >= app.session.notebooks.len()
                || (app.view.transfer_target_notebook == cur && app.session.notebooks.len() > 1)
            {
                app.view.transfer_target_notebook = alt;
            }
            if app.session.notebooks.len() <= 1 && app.view.transfer_merge_whole_notebook.is_none()
            {
                app.view.transfer_create_new_notebook = true;
            }
            if app.view.transfer_new_title.trim().is_empty() {
                app.view.transfer_new_title = format!("{} (Extract)", app.session.doc.title);
            }
        }
    }
    app.view.active_modal = Some(target);
    let initial_step = active_step_index(app, target);
    app.view.motion.modal_flow.sync(
        (app.view.modal_open_seq, target.id_tag(), initial_step),
        initial_step as isize,
    );
    app.view.motion.modal_subflow.reset();
}

/// Applies preset defaults to the New Notebook modal drafts.
pub(crate) fn sync_new_notebook_from_preset(
    app: &mut DrawCraftApp,
    preset: drawcraft_doc::NotebookPreset,
) {
    app.view.new_notebook_preset = preset;
    app.view.new_notebook_title = preset.default_title().to_string();
    app.view.new_notebook_cover = preset.cover();
    app.view.new_notebook_template = preset.template();
    app.view.new_notebook_color = preset.paper_color();
    app.view.new_notebook_format = crate::page_format_picker::PageFormatDraft::default_standard();
    app.view
        .cover_color_draft
        .sync_from_cover(app.view.new_notebook_cover);
}

/// Closes whichever modal dialog is currently active.
pub(crate) fn close(app: &mut DrawCraftApp) {
    app.view.active_modal = None;
    app.view.pending_delete_notebook = None;
    app.view.pending_page_reorder = None;
    app.view.folder_dialog_move_notebook = None;
    app.view.folder_dialog_focus_pending = false;
    app.view.folder_picker_new_open = false;
    app.view.folder_picker_new_name.clear();
    app.view.tag_edit_index = None;
    app.view.tag_edit_draft.clear();
    app.view.tag_edit_focus_pending = false;
    app.view.template_import_error = None;
    app.view.export_target_pages = None;
    app.view.export_feedback = None;
}

#[derive(Clone, Copy, Debug)]
struct ModalMotion {
    eased: f32,
    backdrop_eased: f32,
    interactive: bool,
    animate_flows: bool,
    flow_opacity: f32,
    flow_offset_x: f32,
    dt: f32,
}

impl ModalMotion {
    const STEADY: Self = Self {
        eased: 1.0,
        backdrop_eased: 1.0,
        interactive: true,
        animate_flows: false,
        flow_opacity: 1.0,
        flow_offset_x: 0.0,
        dt: 0.0,
    };
}

/// Renders the active modal dialog if one is open, or smoothly finishes its exit fade.
pub fn show(app: &mut DrawCraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let target = app.view.active_modal;
    let prev_rendered = app.view.motion.modal.rendered;
    let first_open_sizing = prev_rendered.is_none()
        && target.is_some_and(|next| {
            let modal_id = ui.make_persistent_id(("drawcraft_modal_surface", next.id_tag()));
            egui::AreaState::load(&ctx, modal_id).is_none()
        });
    let dt = ctx.input(|i| i.stable_dt);
    let slot_animating = app.view.motion.modal.advance(target, dt, first_open_sizing);
    let any_visible = app.view.motion.modal.rendered.is_some() || target.is_some();
    let backdrop_animating =
        app.view
            .motion
            .modal_backdrop
            .advance(any_visible, dt, first_open_sizing);

    if app.view.motion.modal.rendered != prev_rendered {
        if let Some(next_active) = app.view.motion.modal.rendered {
            let step = active_step_index(app, next_active);
            app.view.motion.modal_flow.sync(
                (app.view.modal_open_seq, next_active.id_tag(), step),
                step as isize,
            );
            app.view.motion.modal_subflow.reset();
        } else {
            app.view.motion.modal_flow.reset();
            app.view.motion.modal_subflow.reset();
        }
    }

    let Some(active) = app.view.motion.modal.rendered else {
        if backdrop_animating {
            ctx.request_repaint();
        }
        return;
    };

    let flow_animating = if target == Some(active) {
        let step = active_step_index(app, active);
        app.view.motion.modal_flow.advance(
            (app.view.modal_open_seq, active.id_tag(), step),
            step as isize,
            dt,
        )
    } else {
        false
    };

    if slot_animating || backdrop_animating || flow_animating {
        ctx.request_repaint();
    }

    let motion = ModalMotion {
        eased: crate::motion::ease_out_cubic(app.view.motion.modal.progress),
        backdrop_eased: app.view.motion.modal_backdrop.eased(),
        interactive: target == Some(active),
        animate_flows: true,
        flow_opacity: app.view.motion.modal_flow.opacity(),
        flow_offset_x: app.view.motion.modal_flow.offset_x(),
        dt,
    };
    let _ = show_inner_with_motion(app, ui, active, motion);
}

#[cfg_attr(not(test), allow(dead_code))]
fn show_inner(app: &mut DrawCraftApp, ui: &mut egui::Ui, active: ActiveModal) -> ModalMetrics {
    show_inner_with_motion(app, ui, active, ModalMotion::STEADY)
}

fn show_inner_with_motion(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    active: ActiveModal,
    motion: ModalMotion,
) -> ModalMetrics {
    match active {
        ActiveModal::Export => show_dialog(
            app,
            ui,
            active,
            motion,
            export_modal::render_body,
            export_modal::render_footer,
        ),
        ActiveModal::NewNotebook => show_dialog(
            app,
            ui,
            active,
            motion,
            customization_modals::render_new_notebook_body,
            customization_modals::render_new_notebook_footer,
        ),
        ActiveModal::NotebookSettings => show_dialog(
            app,
            ui,
            active,
            motion,
            customization_modals::render_notebook_settings_body,
            customization_modals::render_notebook_settings_footer,
        ),
        ActiveModal::ConfirmDeleteNotebook => show_dialog(
            app,
            ui,
            active,
            motion,
            customization_modals::render_delete_notebook_confirmation_body,
            customization_modals::render_delete_notebook_confirmation_footer,
        ),
        ActiveModal::ConfirmReorderPage => show_dialog(
            app,
            ui,
            active,
            motion,
            customization_modals::render_reorder_page_confirmation_body,
            customization_modals::render_reorder_page_confirmation_footer,
        ),
        ActiveModal::AccountSync => show_dialog(
            app,
            ui,
            active,
            motion,
            crate::account_modal::render_body,
            crate::account_modal::render_footer,
        ),
        ActiveModal::FirstAccessNickname => show_dialog(
            app,
            ui,
            active,
            motion,
            crate::account_modal::render_first_access_body,
            crate::account_modal::render_first_access_footer,
        ),
        ActiveModal::SyncConflict => show_dialog(
            app,
            ui,
            active,
            motion,
            crate::account_modal::render_conflict_body,
            crate::account_modal::render_conflict_footer,
        ),
        ActiveModal::TransferPages => show_dialog(
            app,
            ui,
            active,
            motion,
            customization_modals::render_transfer_pages_body,
            customization_modals::render_transfer_pages_footer,
        ),
        ActiveModal::NewFolder => show_dialog(
            app,
            ui,
            active,
            motion,
            folder_modals::render_new_folder_body,
            folder_modals::render_new_folder_footer,
        ),
        ActiveModal::RenameFolder => show_dialog(
            app,
            ui,
            active,
            motion,
            folder_modals::render_rename_folder_body,
            folder_modals::render_rename_folder_footer,
        ),
        ActiveModal::DeleteFolder => show_dialog(
            app,
            ui,
            active,
            motion,
            folder_modals::render_delete_folder_body,
            folder_modals::render_delete_folder_footer,
        ),
        ActiveModal::FolderColor => show_dialog(
            app,
            ui,
            active,
            motion,
            folder_modals::render_folder_color_body,
            folder_modals::render_folder_color_footer,
        ),
        ActiveModal::TagColor => show_dialog(
            app,
            ui,
            active,
            motion,
            folder_modals::render_tag_color_body,
            folder_modals::render_tag_color_footer,
        ),
    }
}

fn modal_steps(active: ActiveModal) -> Option<&'static [ModalStepSpec]> {
    match active {
        ActiveModal::NotebookSettings => Some(NOTEBOOK_SECTION_STEPS),
        ActiveModal::NewNotebook
        | ActiveModal::Export
        | ActiveModal::ConfirmDeleteNotebook
        | ActiveModal::ConfirmReorderPage
        | ActiveModal::AccountSync
        | ActiveModal::FirstAccessNickname
        | ActiveModal::SyncConflict
        | ActiveModal::TransferPages
        | ActiveModal::NewFolder
        | ActiveModal::RenameFolder
        | ActiveModal::DeleteFolder
        | ActiveModal::FolderColor
        | ActiveModal::TagColor => None,
    }
}

fn active_step_index(app: &DrawCraftApp, active: ActiveModal) -> usize {
    match active {
        ActiveModal::NewNotebook => app
            .view
            .new_notebook_step
            .min(NOTEBOOK_SECTION_STEPS.len().saturating_sub(1)),
        ActiveModal::NotebookSettings => app
            .view
            .notebook_settings_step
            .min(NOTEBOOK_SECTION_STEPS.len().saturating_sub(1)),
        _ => 0,
    }
}

fn set_active_step_index(app: &mut DrawCraftApp, active: ActiveModal, step: usize) {
    let clamped = step.min(NOTEBOOK_SECTION_STEPS.len().saturating_sub(1));
    match active {
        ActiveModal::NewNotebook => {
            app.view.new_notebook_step = clamped;
        }
        ActiveModal::NotebookSettings => {
            app.view.notebook_settings_step = clamped;
        }
        _ => {}
    }
}

fn show_dialog(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    active: ActiveModal,
    motion: ModalMotion,
    body: impl FnOnce(&mut DrawCraftApp, &mut egui::Ui, &Tokens) -> bool,
    footer: impl FnOnce(&mut DrawCraftApp, &mut egui::Ui, &Tokens) -> bool,
) -> ModalMetrics {
    let spec = active.spec();
    let tag = active.id_tag();
    let steps = modal_steps(active);
    let current_step = active_step_index(app, active);
    let ctx = ui.ctx().clone();
    let t = Tokens::for_theme(app.session.prefs.theme);
    let viewport = ctx.content_rect();
    let margin = 20_i8;
    let margin_f = f32::from(margin);
    let dialog_width = spec.width.min((viewport.width() - 32.0).max(320.0));
    let inner_w = (dialog_width - margin_f * 2.0 - 2.0).max(260.0);
    let chrome_reserve = if steps.is_some() { 286.0 } else { 240.0 };
    let max_body_height = (viewport.height() - chrome_reserve).max(160.0);

    let backdrop_id = ui.make_persistent_id("drawcraft_modal_backdrop");
    let modal_id = ui.make_persistent_id(("drawcraft_modal_surface", tag));

    let backdrop_resp = egui::Area::new(backdrop_id)
        .order(egui::Order::Middle)
        .fixed_pos(viewport.min)
        .fade_in(false)
        .interactable(motion.interactive)
        .show(&ctx, |ui| {
            let (_rect, resp) = ui.allocate_exact_size(viewport.size(), egui::Sense::click());
            let backdrop_alpha = if t.is_dark { 145 } else { 92 };
            ui.painter().rect_filled(
                viewport,
                0.0,
                Color32::from_black_alpha(backdrop_alpha).gamma_multiply(motion.backdrop_eased),
            );
            resp
        })
        .inner;

    let mut should_close = false;
    let mut close_btn_rect = egui::Rect::NOTHING;
    let mut body_rect = egui::Rect::NOTHING;
    let mut footer_rect = egui::Rect::NOTHING;

    let shadow = ctx.style_of(ctx.theme()).visuals.window_shadow;
    let modal_frame = egui::Frame::NONE
        .fill(t.field_bg)
        .stroke(egui::Stroke::new(1.0, t.border))
        .corner_radius(14.0)
        .inner_margin(egui::Margin::same(margin))
        .shadow(shadow);

    let settle_offset = egui::vec2(
        0.0,
        (1.0 - motion.eased) * crate::motion::MODAL_SETTLE_OFFSET_Y,
    );
    let modal_resp = egui::Area::new(modal_id)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, settle_offset)
        .default_size(egui::vec2(
            dialog_width,
            (viewport.height() - 48.0).max(260.0),
        ))
        .constrain(true)
        .fade_in(false)
        .interactable(motion.interactive)
        .show(&ctx, |ui| {
            ui.multiply_opacity(motion.eased);
            modal_frame.show(ui, |ui| {
                ui.set_min_width(inner_w);
                ui.set_max_width(inner_w);
                ui.spacing_mut().item_spacing.y = 0.0;

                let allow_close = active != ActiveModal::FirstAccessNickname;
                let (close_clicked, x_rect) =
                    draw_dialog_header(ui, &spec, inner_w, allow_close, &t);
                close_btn_rect = x_rect;
                if allow_close && close_clicked {
                    should_close = true;
                }

                if let Some(step_list) = steps {
                    ui.add_space(10.0);
                    let picked = if active == ActiveModal::NotebookSettings {
                        draw_dialog_tabs(
                            ui,
                            app.view.modal_open_seq,
                            tag,
                            step_list,
                            current_step,
                            &t,
                        )
                    } else {
                        draw_dialog_stepper(ui, tag, step_list, current_step, inner_w, &t)
                    };
                    if let Some(picked_step) = picked {
                        set_active_step_index(app, active, picked_step);
                    }
                    ui.add_space(10.0);
                } else {
                    ui.add_space(12.0);
                }
                draw_modal_divider(ui, inner_w, &t);
                ui.add_space(12.0);

                let (body_closed, b_rect) = show_adaptive_body(
                    app,
                    ui,
                    tag,
                    current_step,
                    inner_w,
                    spec.min_body_height,
                    max_body_height,
                    motion,
                    &t,
                    body,
                );
                body_rect = b_rect;
                if body_closed {
                    should_close = true;
                }

                ui.add_space(12.0);
                draw_modal_divider(ui, inner_w, &t);
                ui.add_space(12.0);

                let footer_resp = ui.allocate_ui_with_layout(
                    egui::vec2(inner_w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(inner_w);
                        ui.set_max_width(inner_w);
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                        if footer(app, ui, &t) {
                            should_close = true;
                        }
                    },
                );
                footer_rect = footer_resp.response.rect;
            });
        });

    let modal_rect = modal_resp.response.rect;
    let pointer_in_modal = backdrop_resp
        .interact_pointer_pos()
        .is_some_and(|pos| modal_rect.contains(pos));
    if active != ActiveModal::FirstAccessNickname
        && motion.interactive
        && backdrop_resp.clicked()
        && !pointer_in_modal
    {
        should_close = true;
    }

    if motion.interactive && should_close {
        close(app);
    }

    ModalMetrics {
        modal_rect,
        close_btn_rect,
        body_rect,
        footer_rect,
        inner_margin: margin_f + 1.0,
    }
}

fn draw_modal_divider(ui: &mut egui::Ui, inner_w: f32, t: &Tokens) {
    let (sep_rect, _) = ui.allocate_exact_size(egui::vec2(inner_w, 1.0), egui::Sense::hover());
    ui.painter().hline(
        sep_rect.x_range(),
        sep_rect.center().y,
        egui::Stroke::new(1.0, t.border),
    );
}

#[derive(Clone, Copy)]
struct BodyBox {
    width: f32,
    min_height: f32,
    max_height: f32,
}

fn show_body_content(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    bounds: BodyBox,
    motion: ModalMotion,
    t: &Tokens,
    body: impl FnOnce(&mut DrawCraftApp, &mut egui::Ui, &Tokens) -> bool,
) -> (bool, f32, egui::Rect) {
    let mut closed = false;
    let mut natural_h = 0.0_f32;
    let width = bounds.width;
    let min_height = bounds.min_height;
    let max_height = bounds.max_height;
    let flow_opacity = motion.flow_opacity;
    let flow_offset_x = motion.flow_offset_x;
    let content = ui.allocate_ui_with_layout(
        egui::vec2(width, max_height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.y = 6.0;
            if flow_opacity >= 0.999 && flow_offset_x.abs() <= 0.05 {
                if body(app, ui, t) {
                    closed = true;
                }
                let min_rect = ui.min_rect();
                natural_h = min_rect.height();
                if min_height > natural_h {
                    ui.expand_to_include_rect(egui::Rect::from_min_size(
                        min_rect.min,
                        egui::vec2(width, min_height),
                    ));
                }
            } else {
                let clip_origin = ui.cursor().min;
                let clip = ui.clip_rect().intersect(egui::Rect::from_min_size(
                    clip_origin,
                    egui::vec2(width, max_height + 32.0),
                ));
                let child_max = egui::Rect::from_min_size(
                    clip_origin + egui::vec2(flow_offset_x, 0.0),
                    egui::vec2(width, max_height + 64.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(child_max)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                child.set_clip_rect(clip);
                child.set_width(width);
                child.spacing_mut().item_spacing.y = 6.0;
                child.multiply_opacity(flow_opacity);
                if body(app, &mut child, t) {
                    closed = true;
                }
                natural_h = child.min_rect().height().max(0.0);
                let alloc_h = natural_h.max(min_height);
                let _ = ui.allocate_exact_size(egui::vec2(width, alloc_h), egui::Sense::hover());
            }
        },
    );
    (closed, natural_h, content.response.rect)
}

#[allow(clippy::too_many_arguments)]
fn show_adaptive_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    tag: u8,
    step_idx: usize,
    inner_w: f32,
    base_min_body_height: f32,
    max_body_height: f32,
    motion: ModalMotion,
    t: &Tokens,
    body: impl FnOnce(&mut DrawCraftApp, &mut egui::Ui, &Tokens) -> bool,
) -> (bool, egui::Rect) {
    let height_cache_id = ui.make_persistent_id(("drawcraft_modal_natural_h", tag, step_idx));
    let last_natural_h = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(height_cache_id))
        .unwrap_or(0.0);
    let needs_scroll = last_natural_h > max_body_height + 2.0;

    let base_min = if base_min_body_height > 0.0 {
        base_min_body_height.min(max_body_height)
    } else {
        0.0
    };
    let anim_h_id = ui.make_persistent_id(("drawcraft_modal_anim_h", tag, app.view.modal_open_seq));
    let prev_anim_h = if motion.animate_flows {
        ui.ctx()
            .data(|d| d.get_temp::<f32>(anim_h_id))
            .filter(|h| h.is_finite() && *h > 1.0)
    } else {
        None
    };
    let effective_min_h = if motion.animate_flows {
        prev_anim_h.unwrap_or(base_min).min(max_body_height)
    } else {
        base_min
    };

    let mut closed = false;
    let mut measured_natural_h = 0.0_f32;
    let outer_rect = if needs_scroll {
        ui.spacing_mut().scroll.floating = false;
        let scroll_resp = ui
            .allocate_ui_with_layout(
                egui::vec2(inner_w, max_body_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width(inner_w);
                    ui.set_max_width(inner_w);
                    egui::ScrollArea::vertical()
                        .id_salt(("drawcraft_modal_scroll", tag, step_idx))
                        .min_scrolled_height(effective_min_h)
                        .max_height(max_body_height)
                        .auto_shrink([false, effective_min_h <= 0.0])
                        .show(ui, |ui| {
                            let inner_w_avail = ui.available_width().min(inner_w);
                            ui.set_max_width(inner_w_avail);
                            // Keep enough vertical headroom inside the scrolling body so nested
                            // auto-shrinking ScrollAreas use their full natural height rather than
                            // collapsing to `min_scrolled_size`.
                            let content_max_h = last_natural_h.max(max_body_height + 240.0);
                            let (body_closed, nat_h, _rect) = show_body_content(
                                app,
                                ui,
                                BodyBox {
                                    width: inner_w_avail,
                                    min_height: effective_min_h,
                                    max_height: content_max_h,
                                },
                                motion,
                                t,
                                body,
                            );
                            closed = body_closed;
                            measured_natural_h = nat_h;
                        })
                },
            )
            .inner;
        scroll_resp.inner_rect
    } else {
        let (body_closed, nat_h, rect) = show_body_content(
            app,
            ui,
            BodyBox {
                width: inner_w,
                min_height: effective_min_h,
                max_height: max_body_height,
            },
            motion,
            t,
            body,
        );
        closed = body_closed;
        measured_natural_h = nat_h;
        rect
    };

    ui.ctx()
        .data_mut(|d| d.insert_temp(height_cache_id, measured_natural_h));
    if motion.animate_flows {
        let target_h = measured_natural_h.max(base_min).min(max_body_height);
        let next_h = match prev_anim_h {
            Some(prev) if motion.interactive && (target_h - prev).abs() > 0.5 => {
                let dt = crate::motion::sanitize_dt(motion.dt);
                if dt > 0.0 {
                    let alpha = 1.0 - (-dt / 0.11).exp();
                    let interpolated = prev + (target_h - prev) * alpha;
                    ui.ctx().request_repaint();
                    interpolated
                } else {
                    prev
                }
            }
            _ => target_h,
        };
        ui.ctx().data_mut(|d| d.insert_temp(anim_h_id, next_h));
    }
    (closed, outer_rect)
}

/// Applies directional crossfade + subtle horizontal glide when switching sub-flows inside a modal.
pub(crate) fn apply_subflow_motion(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    modal: ActiveModal,
    subflow_id: u8,
    order: isize,
    add_contents: impl FnOnce(&mut DrawCraftApp, &mut egui::Ui),
) {
    let key = (app.view.modal_open_seq, modal.id_tag(), subflow_id);
    let dt = ui.ctx().input(|i| i.stable_dt);
    let steady =
        app.view.active_modal != Some(modal) && app.view.motion.modal.rendered != Some(modal);
    if steady {
        app.view.motion.modal_subflow.sync(key, order);
    } else if app.view.motion.modal_subflow.advance(key, order, dt) {
        ui.ctx().request_repaint();
    }

    let opacity = app.view.motion.modal_subflow.opacity();
    let offset_x = app.view.motion.modal_subflow.offset_x();
    if opacity >= 0.999 && offset_x.abs() <= 0.05 {
        add_contents(app, ui);
        return;
    }

    let clip = ui.clip_rect();
    let avail_w = ui.available_width().max(1.0);
    let avail_h = ui.available_height().max(400.0);
    let origin = ui.cursor().min;
    let shifted_max = egui::Rect::from_min_size(
        origin + egui::vec2(offset_x, 0.0),
        egui::vec2(avail_w, avail_h),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(shifted_max)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(clip);
    child.set_width(avail_w);
    child.multiply_opacity(opacity);
    add_contents(app, &mut child);
    let measured_h = child.min_rect().height().max(0.0);
    let _ = ui.allocate_exact_size(egui::vec2(avail_w, measured_h), egui::Sense::hover());
}

fn draw_dialog_header(
    ui: &mut egui::Ui,
    spec: &DialogSpec,
    inner_w: f32,
    show_close: bool,
    t: &Tokens,
) -> (bool, egui::Rect) {
    let mut close_clicked = false;
    let mut close_rect = egui::Rect::NOTHING;
    let close_btn_w = 28.0;
    let gap = 10.0;
    let text_w = (inner_w - close_btn_w - gap).max(120.0);

    // Just a title and a quiet close: no badge icon, no subtitle (less is more).
    ui.allocate_ui_with_layout(
        egui::vec2(inner_w, 0.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.allocate_ui_with_layout(
                egui::vec2(text_w, 28.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(text_w);
                    ui.set_max_width(text_w);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(spec.title)
                                .family(egui::FontFamily::Name(crate::theme::DISPLAY_FAMILY.into()))
                                .size(17.0)
                                .color(t.text),
                        )
                        .truncate(),
                    );
                },
            );
            ui.add_space(gap);

            if show_close {
                let resp = icon_button(ui, Icon::Close, None, false, true, t)
                    .on_hover_text("Close dialog (Esc)");
                close_rect = resp.rect;
                if resp.clicked() {
                    close_clicked = true;
                }
            } else {
                let (_, spacer) =
                    ui.allocate_exact_size(egui::vec2(close_btn_w, 28.0), egui::Sense::hover());
                close_rect = spacer.rect;
            }
        },
    );

    (close_clicked, close_rect)
}
fn add_modal_action_button(
    ui: &mut egui::Ui,
    btn: egui::Button<'_>,
    enabled: bool,
) -> egui::Response {
    let resp = ui.add_enabled(enabled, btn);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Renders a prominent blue primary dialog CTA button.
pub(crate) fn primary_action_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    t: &Tokens,
) -> egui::Response {
    let fill = if enabled { t.accent } else { t.border };
    let text_color = if enabled {
        Color32::WHITE
    } else {
        t.text_muted
    };
    let btn = egui::Button::new(
        egui::RichText::new(label)
            .size(12.0)
            .strong()
            .color(text_color),
    )
    .fill(fill)
    .stroke(egui::Stroke::NONE)
    .corner_radius(7.0)
    .min_size(egui::vec2(0.0, 30.0));
    add_modal_action_button(ui, btn, enabled)
}

/// Renders a clean secondary/outlined dialog action button.
pub(crate) fn secondary_action_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    t: &Tokens,
) -> egui::Response {
    let text_color = if enabled { t.text } else { t.text_muted };
    let btn = egui::Button::new(egui::RichText::new(label).size(12.0).color(text_color))
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .corner_radius(7.0)
        .min_size(egui::vec2(0.0, 30.0));
    add_modal_action_button(ui, btn, enabled)
}

/// Renders a subtle danger-accented secondary action button (e.g., closing a notebook).
pub(crate) fn danger_action_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    t: &Tokens,
) -> egui::Response {
    let text_color = if enabled { t.danger_fg } else { t.text_muted };
    let stroke_color = if enabled { t.danger_border } else { t.border };
    let fill = if enabled { t.danger_bg } else { t.toolbar_bg };
    let btn = egui::Button::new(egui::RichText::new(label).size(12.0).color(text_color))
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke_color))
        .corner_radius(7.0)
        .min_size(egui::vec2(0.0, 30.0));
    add_modal_action_button(ui, btn, enabled)
}

fn draw_dialog_tabs(
    ui: &mut egui::Ui,
    modal_seq: u64,
    tag: u8,
    tabs: &[ModalStepSpec],
    current_tab: usize,
    t: &Tokens,
) -> Option<usize> {
    if tabs.is_empty() {
        return None;
    }
    let options: Vec<crate::icons::SwitchOption<'_>> = tabs
        .iter()
        .map(|tab| {
            crate::icons::SwitchOption::new(Some(tab.icon), Some(tab.label), tab.label, true)
        })
        .collect();
    let (picked, _) = crate::icons::segmented_switch(
        ui,
        ("modal_section_tabs", tag, modal_seq),
        &options,
        Some(current_tab.min(options.len().saturating_sub(1))),
        crate::icons::SwitchLayout::EqualWidth,
        t,
    );
    picked
}

fn draw_dialog_stepper(
    ui: &mut egui::Ui,
    tag: u8,
    steps: &[ModalStepSpec],
    current_step: usize,
    inner_w: f32,
    t: &Tokens,
) -> Option<usize> {
    if steps.is_empty() {
        return None;
    }
    let mut picked = None;
    egui::Frame::NONE
        .fill(t.toolbar_bg)
        .stroke(egui::Stroke::new(1.0, t.border))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::symmetric(5, 4))
        .show(ui, |ui| {
            let count = steps.len().max(1);
            let gap = 6.0_f32;
            let track_w = (inner_w - 10.0).max(160.0);
            let pill_w = ((track_w - gap * ((count - 1) as f32)) / (count as f32)).max(80.0);
            let pill_h = 30.0_f32;
            let (track_rect, _) =
                ui.allocate_exact_size(egui::vec2(track_w, pill_h), egui::Sense::hover());

            let mut step_rects = Vec::with_capacity(count);
            let mut responses = Vec::with_capacity(count);
            for (idx, step) in steps.iter().enumerate() {
                let x = track_rect.left() + (pill_w + gap) * idx as f32;
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, track_rect.top()),
                    egui::vec2(pill_w, pill_h),
                );
                let resp = ui
                    .interact(
                        rect,
                        ui.make_persistent_id(("modal_step_pill", tag, idx)),
                        egui::Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(format!("Step {} of {}: {}", idx + 1, count, step.label));
                if resp.clicked() {
                    picked = Some(idx);
                }
                step_rects.push(rect);
                responses.push(resp);
            }

            // Paint completed & hover pill backgrounds first.
            for (idx, rect) in step_rects.iter().enumerate() {
                let is_active = idx == current_step;
                let is_completed = idx < current_step;
                let hover_t = crate::motion::hover_factor(
                    ui.ctx(),
                    ui.make_persistent_id(("modal_step_hov", tag, idx)),
                    responses[idx].hovered() && !is_active,
                );
                let base_fill = if is_completed {
                    t.accent_soft.gamma_multiply(0.42)
                } else {
                    Color32::TRANSPARENT
                };
                let fill = crate::motion::lerp_color(base_fill, t.field_bg, hover_t * 0.72);
                let border_col = if is_completed {
                    t.accent.gamma_multiply(0.45)
                } else if hover_t > 0.05 {
                    t.border.gamma_multiply(hover_t)
                } else {
                    Color32::TRANSPARENT
                };
                if fill != Color32::TRANSPARENT || border_col != Color32::TRANSPARENT {
                    ui.painter().rect(
                        *rect,
                        egui::CornerRadius::same(7),
                        fill,
                        egui::Stroke::new(1.0, border_col),
                        egui::StrokeKind::Inside,
                    );
                }
            }

            // Smoothly sliding active step pill across the stepper track.
            if let Some(&target_r) = step_rects.get(current_step) {
                let pill_track_id = ui.make_persistent_id(("modal_stepper_track", tag));
                let rel_x0 = target_r.left() - track_rect.left();
                let rel_x1 = target_r.right() - track_rect.left();
                let anim_x0 = track_rect.left()
                    + ui.ctx().animate_value_with_time(
                        pill_track_id.with("x0"),
                        rel_x0,
                        crate::motion::MICRO_DURATION_S,
                    );
                let anim_x1 = track_rect.left()
                    + ui.ctx().animate_value_with_time(
                        pill_track_id.with("x1"),
                        rel_x1,
                        crate::motion::MICRO_DURATION_S,
                    );
                let sliding_rect = egui::Rect::from_min_max(
                    egui::pos2(anim_x0, target_r.top()),
                    egui::pos2(anim_x1, target_r.bottom()),
                );
                ui.painter().rect(
                    sliding_rect,
                    egui::CornerRadius::same(7),
                    t.field_bg,
                    egui::Stroke::new(1.2, t.accent),
                    egui::StrokeKind::Inside,
                );
            }

            // Paint circular step badges, icons, and labels crisp on top.
            for (idx, step) in steps.iter().enumerate() {
                let rect = step_rects[idx];
                let is_active = idx == current_step;
                let is_completed = idx < current_step;

                let badge_center = egui::pos2(rect.left() + 18.0, rect.center().y);
                let badge_r = 9.0_f32;
                let badge_rect = egui::Rect::from_center_size(
                    badge_center,
                    egui::vec2(badge_r * 2.0, badge_r * 2.0),
                );
                if is_active {
                    ui.painter().circle_filled(badge_center, badge_r, t.accent);
                    ui.painter().text(
                        badge_center,
                        egui::Align2::CENTER_CENTER,
                        (idx + 1).to_string(),
                        egui::FontId::proportional(10.5),
                        Color32::WHITE,
                    );
                } else if is_completed {
                    ui.painter()
                        .circle_filled(badge_center, badge_r, t.accent_soft);
                    paint_icon(ui.painter(), badge_rect.shrink(3.5), Icon::Check, t.accent);
                } else {
                    ui.painter().circle(
                        badge_center,
                        badge_r,
                        t.field_bg,
                        egui::Stroke::new(1.0, t.border),
                    );
                    ui.painter().text(
                        badge_center,
                        egui::Align2::CENTER_CENTER,
                        (idx + 1).to_string(),
                        egui::FontId::proportional(10.5),
                        t.text_muted,
                    );
                }

                let icon_rect = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 39.0, rect.center().y),
                    egui::vec2(14.0, 14.0),
                );
                let fg = if is_active {
                    t.text
                } else if is_completed {
                    t.accent
                } else {
                    t.text_muted
                };
                paint_icon(ui.painter(), icon_rect, step.icon, fg);
                ui.painter().text(
                    egui::pos2(icon_rect.right() + 6.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    step.label,
                    egui::FontId::proportional(12.0),
                    if is_active { t.text } else { fg },
                );
            }
        });
    picked
}

/// Renders an outlined or primary dialog action button with a small Lucide icon + text label.
#[allow(dead_code)]
pub(crate) fn step_action_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: Icon,
    icon_after: bool,
    primary: bool,
    enabled: bool,
    t: &Tokens,
) -> egui::Response {
    let text_color = if !enabled {
        t.text_muted
    } else if primary {
        Color32::WHITE
    } else {
        t.text
    };
    let fill = if !enabled {
        if primary { t.border } else { t.toolbar_bg }
    } else if primary {
        t.accent
    } else {
        t.toolbar_bg
    };
    let stroke = if primary {
        egui::Stroke::NONE
    } else {
        egui::Stroke::new(1.0, t.border)
    };
    let pad_x = 12.0_f32;
    let icon_sz = 13.0_f32;
    let icon_gap = 5.0_f32;
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(12.0),
        text_color,
    );
    let width = pad_x * 2.0 + icon_sz + icon_gap + galley.size().x;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(width.max(76.0), 30.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let resp = if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    let hovered = enabled && resp.hovered();
    let final_fill = if hovered {
        if primary { t.accent_hover } else { t.field_bg }
    } else {
        fill
    };
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(7),
        final_fill,
        stroke,
        egui::StrokeKind::Inside,
    );

    let total_inner_w = icon_sz + icon_gap + galley.size().x;
    let left_x = rect.center().x - total_inner_w * 0.5;
    let (icon_x, text_x) = if icon_after {
        (left_x + galley.size().x + icon_gap, left_x)
    } else {
        (left_x, left_x + icon_sz + icon_gap)
    };
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(icon_x, rect.center().y - icon_sz * 0.5),
        egui::vec2(icon_sz, icon_sz),
    );
    paint_icon(ui.painter(), icon_rect, icon, text_color);
    ui.painter().galley(
        egui::pos2(text_x, rect.center().y - galley.size().y * 0.5),
        galley,
        text_color,
    );
    resp
}

/// Outcome returned by [`render_stepped_footer`].
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SteppedFooterOutcome {
    pub close_modal: bool,
    pub finish_clicked: bool,
}

/// Standardized stepped modal footer bar: `[left_slot] ... [Cancel] [Back] [Next | Finish]`.
#[allow(dead_code)]
pub(crate) fn render_stepped_footer(
    ui: &mut egui::Ui,
    step: &mut usize,
    total_steps: usize,
    finish_label: &str,
    finish_enabled: bool,
    t: &Tokens,
    left_slot: impl FnOnce(&mut egui::Ui, &Tokens) -> bool,
) -> SteppedFooterOutcome {
    let mut outcome = SteppedFooterOutcome::default();
    let last_step = total_steps.saturating_sub(1);
    *step = (*step).min(last_step);

    ui.horizontal(|ui| {
        if left_slot(ui, t) {
            outcome.close_modal = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if *step < last_step {
                if step_action_button(ui, "Next", Icon::NextPage, true, true, true, t).clicked() {
                    *step += 1;
                }
            } else if step_action_button(
                ui,
                finish_label,
                Icon::Check,
                false,
                true,
                finish_enabled,
                t,
            )
            .clicked()
            {
                outcome.finish_clicked = true;
            }

            if *step > 0
                && step_action_button(ui, "Back", Icon::PrevPage, false, false, true, t).clicked()
            {
                *step = step.saturating_sub(1);
            }

            if secondary_action_button(ui, "Cancel", true, t).clicked() {
                outcome.close_modal = true;
            }
        });
    });

    outcome
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn all_modals_render_centered_with_aligned_close_icon_and_pinned_footer() {
        let modals = [
            ActiveModal::Export,
            ActiveModal::NewNotebook,
            ActiveModal::NotebookSettings,
            ActiveModal::AccountSync,
            ActiveModal::TransferPages,
        ];

        for modal in modals {
            let ctx = egui::Context::default();
            let mut app = DrawCraftApp::default();
            open(&mut app, modal);

            let mut metrics = ModalMetrics {
                modal_rect: egui::Rect::NOTHING,
                close_btn_rect: egui::Rect::NOTHING,
                body_rect: egui::Rect::NOTHING,
                footer_rect: egui::Rect::NOTHING,
                inner_margin: 0.0,
            };
            for _ in 0..5 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    metrics = show_inner(&mut app, ui, modal);
                });
                out.drop_without_applying_deltas();
            }

            let expected_w = modal.spec().width;
            assert!(
                (metrics.modal_rect.width() - expected_w).abs() <= 4.0,
                "{modal:?} width {} differed from expected {expected_w}",
                metrics.modal_rect.width()
            );
            assert!(
                metrics.modal_rect.height() >= 300.0 && metrics.modal_rect.height() <= 740.0,
                "{modal:?} height {} was outside expected unclipped bounds",
                metrics.modal_rect.height()
            );
            assert!(
                (metrics.modal_rect.center().x - 640.0).abs() <= 2.0
                    && (metrics.modal_rect.center().y - 400.0).abs() <= 2.0,
                "{modal:?} center {:?} was not centered cleanly",
                metrics.modal_rect.center()
            );

            // Top-right X close icon sits flush at top-right inner corner.
            let inner_top = metrics.modal_rect.top() + metrics.inner_margin;
            let inner_right = metrics.modal_rect.right() - metrics.inner_margin;
            assert!(
                (metrics.close_btn_rect.top() - inner_top).abs() <= 2.5,
                "{modal:?} close icon top {} not aligned with header top {inner_top}",
                metrics.close_btn_rect.top()
            );
            assert!(
                (metrics.close_btn_rect.right() - inner_right).abs() <= 2.5,
                "{modal:?} close icon right {} not aligned with header right {inner_right}",
                metrics.close_btn_rect.right()
            );

            // Footer is strictly separated below body and stays inside the modal card.
            assert!(
                metrics.footer_rect.top() >= metrics.body_rect.bottom() + 20.0,
                "{modal:?} footer {:?} was not cleanly separated below body {:?}",
                metrics.footer_rect,
                metrics.body_rect
            );
            assert!(
                metrics.footer_rect.right() <= inner_right + 1.0,
                "{modal:?} footer overflowed right margin ({} > {inner_right})",
                metrics.footer_rect.right()
            );
        }

        // Expanded custom cover color studio inside NewNotebook also stays unclipped and centered.
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        open(&mut app, ActiveModal::NewNotebook);
        app.view.new_notebook_cover = drawcraft_doc::NotebookCover::custom_rgb(0x2B, 0x7A, 0x78);
        app.view
            .cover_color_draft
            .sync_from_cover(app.view.new_notebook_cover);
        let mut custom_metrics = ModalMetrics {
            modal_rect: egui::Rect::NOTHING,
            close_btn_rect: egui::Rect::NOTHING,
            body_rect: egui::Rect::NOTHING,
            footer_rect: egui::Rect::NOTHING,
            inner_margin: 0.0,
        };
        for _ in 0..5 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                custom_metrics = show_inner(&mut app, ui, ActiveModal::NewNotebook);
            });
            out.drop_without_applying_deltas();
        }
        assert!(
            custom_metrics.modal_rect.height() <= 740.0,
            "NewNotebook with expanded custom cover studio height {} exceeded bounds",
            custom_metrics.modal_rect.height()
        );
        assert!(
            custom_metrics.footer_rect.top() >= custom_metrics.body_rect.bottom() + 20.0,
            "NewNotebook custom cover footer was not cleanly separated below body"
        );

        // Verify every step across NewNotebook (0..=2) and NotebookSettings (0..=2)
        // stays unclipped, centered, and fits cleanly inside a 1280×800 viewport.
        let stepped_cases = [
            (ActiveModal::NewNotebook, 3usize),
            (ActiveModal::NotebookSettings, 3usize),
        ];
        for (modal, total_steps) in stepped_cases {
            let mut step_heights = Vec::with_capacity(total_steps);
            for step in 0..total_steps {
                let ctx = egui::Context::default();
                let mut app = DrawCraftApp::default();
                let _ = app.session.execute("page.add", &serde_json::json!({}));
                open(&mut app, modal);
                set_active_step_index(&mut app, modal, step);
                app.view
                    .page_setup_format
                    .select_preset(drawcraft_doc::PageResolutionPreset::Custom);
                app.view
                    .new_notebook_format
                    .select_preset(drawcraft_doc::PageResolutionPreset::Custom);

                let mut step_metrics = ModalMetrics {
                    modal_rect: egui::Rect::NOTHING,
                    close_btn_rect: egui::Rect::NOTHING,
                    body_rect: egui::Rect::NOTHING,
                    footer_rect: egui::Rect::NOTHING,
                    inner_margin: 0.0,
                };
                for _ in 0..5 {
                    let raw = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1280.0, 800.0),
                        )),
                        ..Default::default()
                    };
                    let out = ctx.run_ui(raw, |ui| {
                        crate::theme::apply_light_visuals(ui.ctx());
                        step_metrics = show_inner(&mut app, ui, modal);
                    });
                    out.drop_without_applying_deltas();
                }
                assert!(
                    step_metrics.modal_rect.height() >= 300.0
                        && step_metrics.modal_rect.height() <= 740.0,
                    "{modal:?} step {step} height {} out of bounds",
                    step_metrics.modal_rect.height()
                );
                assert!(
                    step_metrics.footer_rect.top() >= step_metrics.body_rect.bottom() + 20.0,
                    "{modal:?} step {step} footer separated check failed"
                );
                step_heights.push(step_metrics.modal_rect.height());
            }
            assert!(
                step_heights.windows(2).all(|w| (w[0] - w[1]).abs() <= 1.0),
                "{modal:?} modal height changed across steps: {step_heights:?}"
            );
        }
    }

    #[test]
    fn stepped_modals_keep_identical_height_when_navigating_across_steps() {
        for (modal, total_steps) in [
            (ActiveModal::NewNotebook, 3usize),
            (ActiveModal::NotebookSettings, 3usize),
        ] {
            let ctx = egui::Context::default();
            let mut app = DrawCraftApp::default();
            let _ = app.session.execute("page.add", &serde_json::json!({}));
            open(&mut app, modal);

            let sequence: Vec<usize> = (0..total_steps)
                .chain((0..total_steps.saturating_sub(1)).rev())
                .collect();
            let mut observed_heights = Vec::with_capacity(sequence.len());
            for step in sequence {
                set_active_step_index(&mut app, modal, step);
                let mut metrics = ModalMetrics {
                    modal_rect: egui::Rect::NOTHING,
                    close_btn_rect: egui::Rect::NOTHING,
                    body_rect: egui::Rect::NOTHING,
                    footer_rect: egui::Rect::NOTHING,
                    inner_margin: 0.0,
                };
                for _ in 0..3 {
                    let raw = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1280.0, 800.0),
                        )),
                        ..Default::default()
                    };
                    let out = ctx.run_ui(raw, |ui| {
                        crate::theme::apply_light_visuals(ui.ctx());
                        metrics = show_inner(&mut app, ui, modal);
                    });
                    out.drop_without_applying_deltas();
                }
                observed_heights.push(metrics.modal_rect.height());
            }
            assert!(
                observed_heights
                    .windows(2)
                    .all(|w| (w[0] - w[1]).abs() <= 1.0),
                "{modal:?} modal resized during step navigation: {observed_heights:?}"
            );
        }
    }

    #[test]
    fn modal_opens_with_coordinated_fade_and_closes_non_interactively() {
        let mut app = DrawCraftApp::default();
        let ctx = egui::Context::default();
        let viewport = egui::vec2(1280.0, 800.0);
        let run_modal = |ctx: &egui::Context, app: &mut DrawCraftApp, t: f64| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewport)),
                time: Some(t),
                predicted_dt: 0.04,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                show(app, ui);
            });
            out.drop_without_applying_deltas();
        };

        open(&mut app, ActiveModal::NewNotebook);
        // First open performs the 1-frame sizing pass at progress 0.0 so modal and backdrop enter together.
        run_modal(&ctx, &mut app, 0.0);
        assert_eq!(
            app.view.motion.modal.rendered,
            Some(ActiveModal::NewNotebook)
        );
        assert_eq!(app.view.motion.modal.progress, 0.0);

        // Intermediate frame advances fade-in smoothly.
        run_modal(&ctx, &mut app, 0.04);
        assert!(app.view.motion.modal.progress > 0.0 && app.view.motion.modal.progress < 1.0);

        let mut t = 0.04;
        for _ in 0..6 {
            t += 0.04;
            run_modal(&ctx, &mut app, t);
        }
        assert_eq!(app.view.motion.modal.progress, 1.0);

        // Closing clears active_modal immediately while rendered dialog fades out softly.
        close(&mut app);
        assert_eq!(app.view.active_modal, None);
        t += 0.04;
        run_modal(&ctx, &mut app, t);
        assert_eq!(
            app.view.motion.modal.rendered,
            Some(ActiveModal::NewNotebook)
        );
        assert!(app.view.motion.modal.progress > 0.0 && app.view.motion.modal.progress < 1.0);

        for _ in 0..5 {
            t += 0.04;
            run_modal(&ctx, &mut app, t);
        }
        assert_eq!(app.view.motion.modal.rendered, None);
        assert_eq!(app.view.motion.modal.progress, 0.0);
    }

    #[test]
    fn modal_swap_preserves_backdrop_and_glides_between_steps() {
        let mut app = DrawCraftApp::default();
        let ctx = egui::Context::default();
        let viewport = egui::vec2(1280.0, 800.0);
        let run_modal = |ctx: &egui::Context, app: &mut DrawCraftApp, t: f64| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, viewport)),
                time: Some(t),
                predicted_dt: 0.04,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                show(app, ui);
            });
            out.drop_without_applying_deltas();
        };

        open(&mut app, ActiveModal::NotebookSettings);
        run_modal(&ctx, &mut app, 0.0);
        let mut t = 0.0;
        for _ in 0..6 {
            t += 0.04;
            run_modal(&ctx, &mut app, t);
        }
        assert_eq!(app.view.motion.modal.progress, 1.0);
        assert_eq!(app.view.motion.modal_backdrop.progress, 1.0);
        assert_eq!(app.view.motion.modal_flow.offset_x(), 0.0);

        // Switching forward 0 -> 1 initiates a right-to-left horizontal glide that settles to 0.
        app.view.notebook_settings_step = 1;
        t += 0.04;
        run_modal(&ctx, &mut app, t);
        assert!(app.view.motion.modal_flow.offset_x() > 0.0);
        for _ in 0..5 {
            t += 0.04;
            run_modal(&ctx, &mut app, t);
        }
        assert_eq!(app.view.motion.modal_flow.offset_x(), 0.0);

        // Switching backward 1 -> 0 glides left-to-right smoothly.
        app.view.notebook_settings_step = 0;
        t += 0.04;
        run_modal(&ctx, &mut app, t);
        assert!(app.view.motion.modal_flow.offset_x() < 0.0);
        for _ in 0..5 {
            t += 0.04;
            run_modal(&ctx, &mut app, t);
        }
        assert_eq!(app.view.motion.modal_flow.offset_x(), 0.0);

        // Swapping directly to ConfirmDeleteNotebook keeps the backdrop steadily dimmed at 1.0.
        app.view.pending_delete_notebook = Some(std::sync::Arc::clone(&app.session.doc));
        open(&mut app, ActiveModal::ConfirmDeleteNotebook);
        for _ in 0..10 {
            t += 0.04;
            run_modal(&ctx, &mut app, t);
            assert_eq!(app.view.motion.modal_backdrop.progress, 1.0);
        }
        assert_eq!(
            app.view.motion.modal.rendered,
            Some(ActiveModal::ConfirmDeleteNotebook)
        );
        assert_eq!(app.view.motion.modal.progress, 1.0);
    }

    #[test]
    fn folder_modals_render_centered_with_pinned_footer() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "notebook.create",
                &serde_json::json!({ "title": "Calc", "folder_path": "School/Math", "tags": ["exam"] }),
            )
            .unwrap();

        let cases = [
            ActiveModal::NewFolder,
            ActiveModal::RenameFolder,
            ActiveModal::DeleteFolder,
            ActiveModal::FolderColor,
            ActiveModal::TagColor,
        ];
        for target in cases {
            match target {
                ActiveModal::NewFolder => folder_modals::open_new_folder(&mut app, "School", None),
                ActiveModal::RenameFolder => {
                    folder_modals::open_rename_folder(&mut app, "School/Math")
                }
                ActiveModal::DeleteFolder => folder_modals::open_delete_folder(&mut app, "School"),
                ActiveModal::FolderColor => {
                    folder_modals::open_folder_color(&mut app, "School/Math")
                }
                ActiveModal::TagColor => folder_modals::open_tag_color(&mut app, "exam"),
                _ => {}
            }
            let ctx = egui::Context::default();
            let mut metrics = ModalMetrics {
                modal_rect: egui::Rect::NOTHING,
                close_btn_rect: egui::Rect::NOTHING,
                body_rect: egui::Rect::NOTHING,
                footer_rect: egui::Rect::NOTHING,
                inner_margin: 0.0,
            };
            for _ in 0..4 {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    crate::theme::apply_light_visuals(ui.ctx());
                    metrics = show_inner(&mut app, ui, target);
                });
                out.drop_without_applying_deltas();
            }
            assert!(
                (metrics.modal_rect.width() - target.spec().width).abs() <= 4.0,
                "{target:?} width mismatch"
            );
            assert!(
                (metrics.modal_rect.center().x - 640.0).abs() <= 2.0
                    && (metrics.modal_rect.center().y - 400.0).abs() <= 2.0,
                "{target:?} was not centered cleanly: {:?}",
                metrics.modal_rect.center()
            );
            assert!(
                metrics.footer_rect.top() >= metrics.body_rect.bottom() + 20.0,
                "{target:?} footer was not cleanly separated below body"
            );
        }
    }

    fn find_unclipped_text_rects(
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

    #[test]
    fn folder_location_list_expands_unclipped_instead_of_shrinking() {
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "library.folder.create",
                &serde_json::json!({ "path": "test" }),
            )
            .unwrap();
        app.session
            .execute(
                "library.folder.create",
                &serde_json::json!({ "path": "test/mk" }),
            )
            .unwrap();

        folder_modals::open_new_folder(&mut app, "test", None);
        let ctx = egui::Context::default();
        let mut small_metrics = ModalMetrics {
            modal_rect: egui::Rect::NOTHING,
            close_btn_rect: egui::Rect::NOTHING,
            body_rect: egui::Rect::NOTHING,
            footer_rect: egui::Rect::NOTHING,
            inner_margin: 0.0,
        };
        let mut last_shapes = Vec::new();
        for _ in 0..4 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                small_metrics = show_inner(&mut app, ui, ActiveModal::NewFolder);
            });
            last_shapes = out.shapes.clone();
            out.drop_without_applying_deltas();
        }

        // studio-style New Folder dialog renders Color & Icon tabs unclipped.
        for label in ["Color", "Icon", "Done", "Cancel"] {
            let rects = find_unclipped_text_rects(&last_shapes, label);
            assert!(
                !rects.is_empty(),
                "expected New Folder modal label '{label}' to be rendered"
            );
            for (text_rect, clip_rect) in rects {
                assert!(
                    text_rect.top() >= clip_rect.top() - 0.5
                        && text_rect.bottom() <= clip_rect.bottom() + 0.5,
                    "label '{label}' text {text_rect:?} was clipped by {clip_rect:?}"
                );
            }
        }

        // Switching to the Icon tab renders cleanly without clipping or shrinking.
        app.view.folder_dialog_tab = 1;
        let mut icon_metrics = small_metrics;
        for _ in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                icon_metrics = show_inner(&mut app, ui, ActiveModal::NewFolder);
            });
            last_shapes = out.shapes.clone();
            out.drop_without_applying_deltas();
        }
        assert!(
            icon_metrics.body_rect.height() >= 220.0,
            "expected Icon tab body height >= 220px, got {:.1}",
            icon_metrics.body_rect.height()
        );
        assert!(
            !find_unclipped_text_rects(&last_shapes, "Icon").is_empty(),
            "expected Icon tab label unclipped"
        );

        // Compact viewport activates outer modal scroll cleanly without height oscillation.
        let mut compact_heights = Vec::new();
        for _ in 0..6 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 380.0),
                )),
                ..Default::default()
            };
            let mut m = icon_metrics;
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                m = show_inner(&mut app, ui, ActiveModal::NewFolder);
            });
            compact_heights.push(m.modal_rect.height());
            assert!(
                m.footer_rect.bottom() <= 380.0,
                "compact modal footer {:?} overflowed viewport",
                m.footer_rect
            );
            out.drop_without_applying_deltas();
        }
        let settled = compact_heights[2..].to_vec();
        assert!(
            settled.windows(2).all(|w| (w[0] - w[1]).abs() <= 1.0),
            "compact modal height oscillated across frames: {compact_heights:?}"
        );
    }

    #[test]
    fn confirm_reorder_page_modal_renders_centered_with_pinned_footer() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        let _ = app.session.execute("page.add", &serde_json::json!({}));
        let _ = app.session.execute("page.add", &serde_json::json!({}));
        app.view.pending_page_reorder = Some(crate::PendingPageReorder {
            notebook_index: app.session.active_notebook,
            from: 2,
            to: 0,
        });
        open(&mut app, ActiveModal::ConfirmReorderPage);

        let mut metrics = ModalMetrics {
            modal_rect: egui::Rect::NOTHING,
            close_btn_rect: egui::Rect::NOTHING,
            body_rect: egui::Rect::NOTHING,
            footer_rect: egui::Rect::NOTHING,
            inner_margin: 0.0,
        };
        let mut saw_prompt = false;
        for _ in 0..4 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                metrics = show_inner(&mut app, ui, ActiveModal::ConfirmReorderPage);
            });
            saw_prompt |=
                !find_unclipped_text_rects(&out.shapes, "Move Page 3 to position 1?").is_empty();
            out.drop_without_applying_deltas();
        }
        assert!(saw_prompt);
        assert!(
            (metrics.modal_rect.width() - ActiveModal::ConfirmReorderPage.spec().width).abs()
                <= 4.0
        );
        assert!((metrics.modal_rect.center().x - 640.0).abs() <= 2.0);
        assert!((metrics.modal_rect.center().y - 400.0).abs() <= 2.0);
        assert!(metrics.footer_rect.top() >= metrics.body_rect.bottom() + 20.0);
    }
}
