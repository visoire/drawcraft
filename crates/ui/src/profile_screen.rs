//! Dedicated full-window Profile, Account Security, Sync & Workspace screen.

use drawcraft_sync::{SyncConnectionState, SyncStatusSummary};
use egui::{CornerRadius, Margin, Sense, Stroke, Vec2};

use crate::DrawCraftApp;
use crate::account_modal::{
    BannerTone, PillIcon, PillStyle, ProfileSection, SignedOutProfileSection, draw_hairline,
    draw_pill_badge, draw_section_card, draw_status_banner, handle_auth_submit_on_enter,
    render_account_email_card, render_auth_banners, render_auth_card_contents,
    render_change_password_card, render_detected_local_accounts_migration_card,
    render_local_profiles_manager, render_logout_safeguard_actions, render_logout_safeguard_body,
    render_profile_identity_card, render_server_probe_section, render_session_sign_out_card,
    render_signed_in_account_card, render_signed_in_server_card, render_signed_in_transfer_section,
    trigger_test_connection,
};
use crate::icons::{Icon, SwitchLayout, SwitchOption, icon_button, paint_icon, segmented_switch};
use crate::modal::{primary_action_button, secondary_action_button};
use crate::sync_worker::trigger_manual_sync;
use crate::theme::Tokens;

const HEADER_ROW_HEIGHT: f32 = 28.0;
const MAX_CONTENT_WIDTH: f32 = 940.0;
const RAIL_WIDTH: f32 = 228.0;
const COMPACT_BREAKPOINT: f32 = 760.0;

/// Renders the dedicated Profile & Cloud Sync screen.
pub fn show(app: &mut DrawCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::for_theme(app.session.prefs.theme);
    let summary = app.session.sync_status_summary();
    let cloud_pending = app.session.cloud_pending_changes_count();

    draw_profile_header(app, ui, &summary, cloud_pending, &t);
    crate::inspector::show(app, ui);

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(t.panel_bg))
        .show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            let side_pad = 24.0_f32;
            let content_w = (avail.width() - side_pad * 2.0).clamp(280.0, MAX_CONTENT_WIDTH);
            let left = avail.left() + ((avail.width() - content_w) * 0.5).max(side_pad);
            let inner_rect = egui::Rect::from_min_max(
                egui::pos2(left, avail.top() + 20.0),
                egui::pos2(
                    left + content_w,
                    (avail.bottom() - 16.0).max(avail.top() + 60.0),
                ),
            );

            ui.scope_builder(egui::UiBuilder::new().max_rect(inner_rect), |ui| {
                ui.set_width(content_w);
                if content_w >= COMPACT_BREAKPOINT {
                    draw_two_column_layout(app, ui, &summary, cloud_pending, content_w, &t);
                } else {
                    draw_compact_layout(app, ui, &summary, cloud_pending, &t);
                }
            });
        });
}

fn draw_profile_header(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    cloud_pending: usize,
    t: &Tokens,
) {
    let frame = egui::Frame::new()
        .fill(t.toolbar_bg)
        .inner_margin(Margin::symmetric(12, 8))
        .stroke(Stroke::new(1.0, t.border));

    egui::Panel::top("drawcraft_profile_header")
        .frame(frame)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_height(HEADER_ROW_HEIGHT);
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);

                let back_label = if app.view.profile_return_screen == crate::AppScreen::Editor {
                    "Back to Notebook"
                } else {
                    "Back to Library"
                };
                if icon_button(ui, Icon::PrevPage, Some(back_label), false, true, t)
                    .on_hover_text("Return (Esc)")
                    .clicked()
                {
                    crate::close_profile_screen(app);
                }

                ui.separator();

                ui.label(
                    egui::RichText::new("Settings")
                        .family(egui::FontFamily::Name(crate::theme::DISPLAY_FAMILY.into()))
                        .size(16.0)
                        .color(t.text),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.set_height(HEADER_ROW_HEIGHT);
                    ui.spacing_mut().item_spacing.x = 6.0;

                    if summary.signed_in {
                        let in_flight = app.session.is_sync_in_flight();
                        let can_sync = !in_flight && summary.viewing_cloud;
                        let sync_lbl = if in_flight { "Syncing..." } else { "Sync Now" };
                        if primary_action_button(ui, sync_lbl, can_sync, t)
                            .on_hover_text(if summary.viewing_cloud {
                                format!("Sync cloud workspace ({cloud_pending} pending)")
                            } else {
                                "Switch to Cloud Workspace to sync".to_string()
                            })
                            .clicked()
                        {
                            app.view.account.error_message = None;
                            app.view.account.status_message = None;
                            trigger_manual_sync(app, ui.ctx());
                        }
                    }

                    let dot_color = crate::home::workspace_status_dot_color(app, t);
                    let status_label = if !summary.signed_in || !summary.viewing_cloud {
                        "Local Only"
                    } else {
                        match summary.connection {
                            SyncConnectionState::Synced => "Cloud Synced",
                            SyncConnectionState::Syncing => "Syncing",
                            SyncConnectionState::Offline | SyncConnectionState::Error => {
                                "Cloud Failed"
                            }
                            SyncConnectionState::SignedOut => "Local Only",
                        }
                    };
                    draw_pill_badge(
                        ui,
                        PillStyle {
                            fill: t.toolbar_bg,
                            border: t.border,
                            fg: dot_color,
                            icon: PillIcon::Dot,
                        },
                        status_label,
                    );
                });
            });
        });
}

fn draw_two_column_layout(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    cloud_pending: usize,
    content_w: f32,
    t: &Tokens,
) {
    let gap = 18.0_f32;
    let body_w = (content_w - RAIL_WIDTH - gap).max(280.0);
    let max_h = ui.available_height().max(180.0);

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        ui.allocate_ui_with_layout(
            Vec2::new(RAIL_WIDTH, max_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(RAIL_WIDTH);
                draw_sidebar_rail(app, ui, summary, t);
            },
        );

        ui.allocate_ui_with_layout(
            Vec2::new(body_w, max_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(body_w);
                egui::ScrollArea::vertical()
                    .id_salt("drawcraft_profile_body_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width().min(body_w));
                        draw_active_section_content(app, ui, summary, cloud_pending, t);
                        ui.add_space(16.0);
                    });
            },
        );
    });
}

fn draw_compact_layout(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    cloud_pending: usize,
    t: &Tokens,
) {
    if summary.signed_in {
        let options = [
            SwitchOption::new(
                Some(Icon::Palette),
                Some("Studio"),
                "Appearance & study",
                true,
            ),
            SwitchOption::new(
                Some(Icon::FountainPen),
                Some("Pen"),
                "Stylus & smoothing",
                true,
            ),
            SwitchOption::new(
                Some(Icon::Inbox),
                Some("Local"),
                "Local & cloud workspaces",
                true,
            ),
            SwitchOption::new(
                Some(Icon::Account),
                Some("Account"),
                "Profile, security & sync",
                true,
            ),
        ];
        let active = match app.view.account.profile_section {
            ProfileSection::Appearance => 0,
            ProfileSection::Pen | ProfileSection::Telemetry => 1,
            ProfileSection::Workspaces => 2,
            ProfileSection::Profile | ProfileSection::Security | ProfileSection::SyncServer => 3,
        };
        let (clicked, _) = segmented_switch(
            ui,
            "profile_compact_tabs_signed_in",
            &options,
            Some(active),
            SwitchLayout::EqualWidth,
            t,
        );
        if let Some(idx) = clicked {
            app.view.account.profile_section = match idx {
                0 => ProfileSection::Appearance,
                1 => ProfileSection::Pen,
                2 => ProfileSection::Workspaces,
                _ => ProfileSection::Profile,
            };
        }
    } else {
        let options = [
            SwitchOption::new(
                Some(Icon::Palette),
                Some("Studio"),
                "Appearance, study & scale",
                true,
            ),
            SwitchOption::new(
                Some(Icon::FountainPen),
                Some("Pen"),
                "Pen stabilization & stylus",
                true,
            ),
            SwitchOption::new(
                Some(Icon::Inbox),
                Some("Local"),
                "Offline local workspaces",
                true,
            ),
            SwitchOption::new(
                Some(Icon::Server),
                Some("Cloud"),
                "Sign in or create account",
                true,
            ),
        ];
        let active = match app.view.account.signed_out_section {
            SignedOutProfileSection::Appearance => 0,
            SignedOutProfileSection::Pen | SignedOutProfileSection::Telemetry => 1,
            SignedOutProfileSection::LocalWorkspaces => 2,
            SignedOutProfileSection::CloudAccount => 3,
        };
        let (clicked, _) = segmented_switch(
            ui,
            "profile_compact_tabs_signed_out",
            &options,
            Some(active),
            SwitchLayout::EqualWidth,
            t,
        );
        if let Some(idx) = clicked {
            app.view.account.signed_out_section = match idx {
                0 => SignedOutProfileSection::Appearance,
                1 => SignedOutProfileSection::Pen,
                2 => SignedOutProfileSection::LocalWorkspaces,
                _ => SignedOutProfileSection::CloudAccount,
            };
        }
    }

    ui.add_space(12.0);
    egui::ScrollArea::vertical()
        .id_salt("drawcraft_profile_compact_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            draw_active_section_content(app, ui, summary, cloud_pending, t);
            ui.add_space(16.0);
        });
}

fn draw_sidebar_rail(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) {
    draw_identity_summary_card(ui, summary, t);
    ui.add_space(10.0);

    egui::Frame::new()
        .fill(t.toolbar_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            if summary.signed_in {
                let items = [
                    (
                        ProfileSection::Appearance,
                        Icon::Palette,
                        "Appearance & Study",
                        "Theme, iPadOS writing & app scale",
                    ),
                    (
                        ProfileSection::Pen,
                        Icon::FountainPen,
                        "Pen & Stylus",
                        "Stabilization & Apple Pencil",
                    ),
                    (
                        ProfileSection::Telemetry,
                        Icon::Telemetry,
                        "Telemetry & Keys",
                        "Live stylus data & shortcuts",
                    ),
                    (
                        ProfileSection::Workspaces,
                        Icon::Inbox,
                        "Local & Workspaces",
                        "Cloud & local profiles",
                    ),
                    (
                        ProfileSection::Profile,
                        Icon::Account,
                        "Profile",
                        "Display name, @username & email",
                    ),
                    (
                        ProfileSection::Security,
                        Icon::Reset,
                        "Security",
                        "Password & active session",
                    ),
                    (
                        ProfileSection::SyncServer,
                        Icon::Sync,
                        "Sync & Server",
                        "Sync state & server URL",
                    ),
                ];
                for (sec, icon, title, subtitle) in items {
                    let active = app.view.account.profile_section == sec;
                    if draw_rail_nav_button(ui, icon, title, subtitle, active, t) {
                        app.view.account.profile_section = sec;
                    }
                }
            } else {
                let items = [
                    (
                        SignedOutProfileSection::Appearance,
                        Icon::Palette,
                        "Appearance & Study",
                        "Theme, iPadOS writing & app scale",
                    ),
                    (
                        SignedOutProfileSection::Pen,
                        Icon::FountainPen,
                        "Pen & Stylus",
                        "Stabilization & Apple Pencil",
                    ),
                    (
                        SignedOutProfileSection::Telemetry,
                        Icon::Telemetry,
                        "Telemetry & Keys",
                        "Live stylus data & shortcuts",
                    ),
                    (
                        SignedOutProfileSection::LocalWorkspaces,
                        Icon::Inbox,
                        "Local Workspaces",
                        "Offline profiles on this device",
                    ),
                    (
                        SignedOutProfileSection::CloudAccount,
                        Icon::Server,
                        "Cloud Account",
                        "Sign in, sign up, or sync",
                    ),
                ];
                for (sec, icon, title, subtitle) in items {
                    let active = app.view.account.signed_out_section == sec;
                    if draw_rail_nav_button(ui, icon, title, subtitle, active, t) {
                        app.view.account.signed_out_section = sec;
                    }
                }
            }
        });
}

fn draw_identity_summary_card(ui: &mut egui::Ui, summary: &SyncStatusSummary, t: &Tokens) {
    egui::Frame::new()
        .fill(t.toolbar_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;

            let (primary, secondary, monogram) = if summary.signed_in {
                let disp = summary
                    .display_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .or(summary.username.as_deref())
                    .unwrap_or("Account")
                    .to_string();
                let handle = summary
                    .username
                    .as_deref()
                    .map_or_else(String::new, |u| format!("@{u}"));
                let mono = disp
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map_or_else(|| "U".to_string(), |c| c.to_uppercase().collect());
                (disp, handle, mono)
            } else {
                let nick = summary
                    .active_local_profile_name
                    .clone()
                    .unwrap_or_else(|| "Personal".to_string());
                let mono = nick
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map_or_else(|| "L".to_string(), |c| c.to_uppercase().collect());
                (nick, String::new(), mono)
            };

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (avatar_rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                ui.painter()
                    .circle_filled(avatar_rect.center(), 20.0, t.accent_soft);
                ui.painter().circle_stroke(
                    avatar_rect.center(),
                    20.0,
                    Stroke::new(1.2, t.accent.gamma_multiply(0.45)),
                );
                ui.painter().text(
                    avatar_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    monogram,
                    egui::FontId::proportional(16.0),
                    t.accent,
                );

                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(primary)
                                .size(13.5)
                                .strong()
                                .color(t.text),
                        )
                        .wrap(),
                    );
                    if !secondary.is_empty() {
                        ui.label(
                            egui::RichText::new(secondary)
                                .size(11.5)
                                .monospace()
                                .color(t.text_muted),
                        );
                    }
                });
            });

            if let Some(email) = summary.email.as_deref().filter(|s| !s.trim().is_empty()) {
                draw_hairline(ui, t);
                ui.add(
                    egui::Label::new(egui::RichText::new(email).size(11.0).color(t.text_muted))
                        .wrap(),
                );
            }
        });
}

fn draw_rail_nav_button(
    ui: &mut egui::Ui,
    icon: Icon,
    title: &str,
    subtitle: &str,
    selected: bool,
    t: &Tokens,
) -> bool {
    let fill = if selected {
        t.accent_soft
    } else {
        egui::Color32::TRANSPARENT
    };
    let border = Stroke::NONE;
    let title_color = t.text;

    let resp = egui::Frame::new()
        .fill(fill)
        .stroke(border)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                paint_icon(
                    ui.painter(),
                    ir,
                    icon,
                    if selected { t.accent } else { t.text_muted },
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .size(12.5)
                                .strong()
                                .color(title_color),
                        )
                        .selectable(false),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(subtitle).size(10.5).color(t.text_muted),
                        )
                        .selectable(false)
                        .wrap(),
                    );
                });
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.clicked()
}

fn draw_active_section_content(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    cloud_pending: usize,
    t: &Tokens,
) {
    ui.spacing_mut().item_spacing.y = 0.0;

    if summary.signed_in && app.view.account.logout_confirm_open {
        render_logout_safeguard_body(app, ui, t);
        ui.add_space(10.0);
        draw_section_card(ui, t, |ui| {
            render_logout_safeguard_actions(app, ui, t);
        });
        return;
    }

    if summary.signed_in {
        match app.view.account.profile_section {
            ProfileSection::Profile => {
                draw_section_card(ui, t, |ui| {
                    render_profile_identity_card(app, ui, t);
                });
                ui.add_space(10.0);
                draw_section_card(ui, t, |ui| {
                    render_account_email_card(app, ui, summary, t);
                });
            }
            ProfileSection::Security => {
                draw_section_card(ui, t, |ui| {
                    render_change_password_card(app, ui, t);
                });
                ui.add_space(10.0);
                draw_section_card(ui, t, |ui| {
                    render_session_sign_out_card(app, ui, summary, t);
                });
            }
            ProfileSection::SyncServer => {
                draw_section_card(ui, t, |ui| {
                    render_signed_in_account_card(app, ui, summary, cloud_pending, t);
                });
                if !app.session.accessible_local_profiles().is_empty() {
                    ui.add_space(10.0);
                    draw_section_card(ui, t, |ui| {
                        render_detected_local_accounts_migration_card(app, ui, summary, t);
                    });
                }
                ui.add_space(10.0);
                let mut test_on_enter = false;
                draw_section_card(ui, t, |ui| {
                    test_on_enter = render_signed_in_server_card(app, ui, summary, t);
                });
                if test_on_enter && !app.view.account.is_testing() {
                    trigger_test_connection(app, ui.ctx());
                }
            }
            ProfileSection::Workspaces => {
                draw_signed_in_workspaces_section(app, ui, summary, t);
            }
            ProfileSection::Appearance => {
                crate::inspector::draw_appearance_card(app, ui, t);
                ui.add_space(10.0);
                crate::study_ui::draw_study_prefs_card(app, ui, t);
                ui.add_space(10.0);
                crate::inspector::draw_app_scale_card(app, ui, t);
            }
            ProfileSection::Pen => {
                let kind = app.session.active_pen_kind();
                crate::inspector::draw_live_stroke_preview(app, ui, kind, t);
                ui.add_space(10.0);
                crate::inspector::draw_pen_smoothing_card(app, ui, kind, t);
                if app.is_stylus_connected() {
                    ui.add_space(10.0);
                    crate::inspector::draw_stylus_shortcuts_card(app, ui, t);
                }
            }
            ProfileSection::Telemetry => {
                crate::inspector::draw_live_telemetry_card(app, ui, t);
                ui.add_space(10.0);
                crate::inspector::draw_shortcuts_card(ui, t);
            }
        }
    } else {
        match app.view.account.signed_out_section {
            SignedOutProfileSection::CloudAccount => {
                ui.label(
                    egui::RichText::new("Profile & Cloud Sync")
                        .size(14.5)
                        .strong()
                        .color(t.text),
                );
                ui.add_space(8.0);
                let mut test_on_enter = false;
                draw_section_card(ui, t, |ui| {
                    test_on_enter = render_server_probe_section(app, ui, t, true);
                });
                ui.add_space(10.0);
                let mut submit_on_enter = false;
                draw_section_card(ui, t, |ui| {
                    submit_on_enter = render_auth_card_contents(app, ui, t, true);
                });
                if test_on_enter && !app.view.account.is_testing() {
                    trigger_test_connection(app, ui.ctx());
                }
                if submit_on_enter {
                    handle_auth_submit_on_enter(app, ui.ctx());
                }
            }
            SignedOutProfileSection::LocalWorkspaces => {
                draw_section_card(ui, t, |ui| {
                    let active_name = summary
                        .active_local_profile_name
                        .as_deref()
                        .unwrap_or("Personal");
                    let nb_count = app.session.notebooks.len();
                    ui.horizontal(|ui| {
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_icon(ui.painter(), ir, Icon::Inbox, t.accent);
                        ui.label(
                            egui::RichText::new(format!("On This Device: {active_name}"))
                                .size(13.5)
                                .strong()
                                .color(t.text),
                        );
                    });
                    ui.add_space(3.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "{nb_count} notebook(s) in this local workspace. Local profiles stay completely offline until you sign in or transfer notebooks."
                            ))
                            .size(11.0)
                            .color(t.text_muted),
                        )
                        .wrap(),
                    );
                    ui.add_space(10.0);
                    draw_hairline(ui, t);
                    ui.add_space(10.0);
                    render_local_profiles_manager(app, ui, t);
                    ui.add_space(10.0);
                    draw_hairline(ui, t);
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if secondary_action_button(ui, "Export Backup (.dcraftpkg)", true, t)
                            .clicked()
                        {
                            let _ = app.trigger_export_package_dialog();
                        }
                        if secondary_action_button(ui, "Import Backup (.dcraftpkg)", true, t)
                            .clicked()
                        {
                            let _ = app.trigger_import_package_dialog();
                        }
                    });
                });
            }
            SignedOutProfileSection::Appearance => {
                crate::inspector::draw_appearance_card(app, ui, t);
                ui.add_space(10.0);
                crate::study_ui::draw_study_prefs_card(app, ui, t);
                ui.add_space(10.0);
                crate::inspector::draw_app_scale_card(app, ui, t);
            }
            SignedOutProfileSection::Pen => {
                let kind = app.session.active_pen_kind();
                crate::inspector::draw_live_stroke_preview(app, ui, kind, t);
                ui.add_space(10.0);
                crate::inspector::draw_pen_smoothing_card(app, ui, kind, t);
                if app.is_stylus_connected() {
                    ui.add_space(10.0);
                    crate::inspector::draw_stylus_shortcuts_card(app, ui, t);
                }
            }
            SignedOutProfileSection::Telemetry => {
                crate::inspector::draw_live_telemetry_card(app, ui, t);
                ui.add_space(10.0);
                crate::inspector::draw_shortcuts_card(ui, t);
            }
        }
    }

    if let Some(ref conflict) = summary.last_conflict_notice {
        ui.add_space(8.0);
        draw_status_banner(ui, BannerTone::Info, conflict, t);
    }
    if let Some(ref err) = summary.last_error
        && app.view.account.error_message.is_none()
        && summary.connection == SyncConnectionState::Error
    {
        ui.add_space(8.0);
        draw_status_banner(ui, BannerTone::Error, err, t);
    }
    render_auth_banners(app, ui, t);
}

fn draw_signed_in_workspaces_section(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) {
    draw_section_card(ui, t, |ui| {
        let acct_label = summary
            .email
            .clone()
            .or_else(|| summary.username.as_ref().map(|u| format!("@{u}")))
            .unwrap_or_else(|| "Cloud Account".to_string());
        ui.horizontal(|ui| {
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            paint_icon(ui.painter(), ir, Icon::Sync, t.accent);
            ui.label(
                egui::RichText::new(format!("Cloud Workspace · {acct_label}"))
                    .size(13.5)
                    .strong()
                    .color(t.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if summary.viewing_cloud {
                    draw_pill_badge(
                        ui,
                        PillStyle {
                            fill: t.success_bg,
                            border: t.success_border,
                            fg: t.success_fg,
                            icon: PillIcon::GreenCheck(t),
                        },
                        "Active",
                    );
                } else if secondary_action_button(ui, "Switch to Cloud", true, t).clicked() {
                    crate::canvas::commit_pending_text(app);
                    if let Err(err) = app
                        .session
                        .execute("profile.switch", &serde_json::json!({ "target": "cloud" }))
                    {
                        app.view.account.error_message = Some(err.to_string());
                    } else {
                        app.view.account.error_message = None;
                        app.view.profile_return_screen = crate::AppScreen::Home;
                        crate::sync_view_to_active_notebook(app);
                    }
                }
            });
        });
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Switch between Cloud Sync and isolated Local Only profiles on this computer.",
                )
                .size(11.0)
                .color(t.text_muted),
            )
            .wrap(),
        );
        ui.add_space(10.0);
        draw_hairline(ui, t);
        ui.add_space(10.0);
        render_local_profiles_manager(app, ui, t);
    });

    ui.add_space(10.0);
    draw_section_card(ui, t, |ui| {
        app.view.account.import_local_open = true;
        render_signed_in_transfer_section(app, ui, t);
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::AppScreen;
    use crate::account_modal::AuthTab;
    use crate::sync_worker::{SyncWorkerEvent, apply_worker_event};
    use drawcraft_engine::AuthWorkspaceMode;
    use drawcraft_sync::ActiveAccountSession;
    use drawcraft_sync_protocol::{DEFAULT_SERVER_URL, UserProfile};
    use egui::{Shape, epaint::ClippedShape};

    fn collect_text(shapes: &[ClippedShape]) -> String {
        let mut out = String::new();
        fn visit(shape: &Shape, buf: &mut String) {
            match shape {
                Shape::Text(text) => {
                    buf.push_str(text.galley.text());
                    buf.push('\n');
                }
                Shape::Vec(items) => {
                    for item in items {
                        visit(item, buf);
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

    fn render_profile_text(ctx: &egui::Context, app: &mut DrawCraftApp) -> String {
        let mut text = String::new();
        for step in 0..4 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(f64::from(step) * 0.05),
                predicted_dt: 0.05,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_visuals(ui.ctx(), app.session.prefs.theme);
                show(app, ui);
            });
            text = collect_text(&out.shapes);
            out.drop_without_applying_deltas();
        }
        text
    }

    #[test]
    fn signed_out_profile_screen_transitions_into_password_reset_flow() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        crate::open_profile_screen(&mut app);
        assert_eq!(app.view.screen, AppScreen::Profile);

        app.view.account.tab = AuthTab::CreateAccount;
        let signup_text = render_profile_text(&ctx, &mut app);
        assert!(signup_text.contains("Profile & Cloud Sync"));
        assert!(signup_text.contains("Username"));
        assert!(signup_text.contains("Display Name"));

        app.view.account.tab = AuthTab::SignIn;
        apply_worker_event(
            &mut app,
            &ctx,
            SyncWorkerEvent::PasswordResetRequired {
                identifier: "maria@example.com".to_string(),
                prefill_code: Some("ABCD-EFGH-JKLM".to_string()),
                message: "Temporary password verified. Choose a new password.".to_string(),
            },
        );
        assert!(app.view.account.password_reset.active);
        assert_eq!(
            app.view.account.password_reset.recovery_code,
            "ABCD-EFGH-JKLM"
        );
        let reset_text = render_profile_text(&ctx, &mut app);
        assert!(reset_text.contains("Complete Password Reset"));
        assert!(reset_text.contains("Temporary Recovery Code"));
        assert!(reset_text.contains("Set New Password & Sign In"));
    }

    #[test]
    fn signed_in_profile_screen_renders_all_sections_in_light_and_dark() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.session.switch_to_authenticated_library(
            ActiveAccountSession {
                server_url: DEFAULT_SERVER_URL.to_string(),
                user: UserProfile {
                    user_id: "u-prof-1".to_string(),
                    email: Some("alex@example.com".to_string()),
                    username: "alex_notes".to_string(),
                    display_name: "Alex Silva".to_string(),
                    created_at_ms: 1000,
                    must_change_password: false,
                },
                access_token: "acc".to_string(),
                refresh_token: "ref".to_string(),
                access_expires_at_ms: 99_999,
                refresh_expires_at_ms: 99_999,
                device_name: "Desk".to_string(),
                last_auth_ms: 1000,
            },
            AuthWorkspaceMode::SwitchOnly,
        );
        crate::open_profile_screen(&mut app);

        app.view.account.profile_section = ProfileSection::Profile;
        let profile_text = render_profile_text(&ctx, &mut app);
        assert!(profile_text.contains("Profile Details"));
        assert!(profile_text.contains("Save Profile"));
        assert!(profile_text.contains("Email Address"));
        assert!(profile_text.contains("@alex_notes"));

        app.view.account.profile_section = ProfileSection::Security;
        let _ = app
            .session
            .execute("theme.set", &serde_json::json!({ "theme": "Dark" }));
        let sec_text = render_profile_text(&ctx, &mut app);
        assert!(sec_text.contains("Change Password"));
        assert!(sec_text.contains("Update Password"));
        assert!(sec_text.contains("Session & Sign Out"));

        app.view.account.profile_section = ProfileSection::SyncServer;
        let sync_text = render_profile_text(&ctx, &mut app);
        assert!(sync_text.contains("Server Address"));
        assert!(sync_text.contains("PENDING UPLOADS"));

        app.view.account.profile_section = ProfileSection::Workspaces;
        let ws_text = render_profile_text(&ctx, &mut app);
        assert!(ws_text.contains("Cloud Workspace"));

        crate::close_profile_screen(&mut app);
        assert_eq!(app.view.screen, AppScreen::Home);
    }
}
