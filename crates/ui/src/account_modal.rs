//! Account sign-in, local profile isolation, email linking, sync status, and logout safeguards.

use drawcraft_engine::AuthWorkspaceMode;
use drawcraft_sync::{
    EntitySyncStatus, SyncConflictPreview, SyncConnectionState, SyncStatusSummary,
};
use drawcraft_sync_protocol::{
    DEFAULT_SERVER_URL, MIN_PASSWORD_LEN, ServerInfoResponse, normalize_email,
    normalize_server_url, normalize_username, now_unix_ms, sanitize_display_name,
    validate_password,
};
use egui::{Color32, CornerRadius, Margin, Rect, Sense, Stroke, StrokeKind, Vec2};
use serde_json::json;

use crate::DrawCraftApp;
use crate::icons::{Icon, SwitchLayout, SwitchOption, paint_icon, segmented_switch};
use crate::modal::{danger_action_button, primary_action_button, secondary_action_button};
use crate::sync_worker::{
    SyncWorkerCommand, complete_sign_out, trigger_conflict_keep_local,
    trigger_conflict_keep_server, trigger_manual_sync,
};
use crate::theme::Tokens;

const INPUT_HEIGHT: f32 = 30.0;
const CARD_RADIUS: f32 = 10.0;
const CONTROL_RADIUS: f32 = 7.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthTab {
    #[default]
    SignIn,
    CreateAccount,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProfileSection {
    #[default]
    Profile,
    Security,
    SyncServer,
    Workspaces,
    Appearance,
    Pen,
    Telemetry,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignedOutProfileSection {
    #[default]
    CloudAccount,
    LocalWorkspaces,
    Appearance,
    Pen,
    Telemetry,
}

#[derive(Clone, Debug)]
pub struct PasswordResetFlowState {
    pub active: bool,
    pub identifier: String,
    pub recovery_code: String,
    pub show_recovery_code: bool,
    pub new_password: String,
    pub confirm_password: String,
    pub show_new_password: bool,
    pub busy: bool,
    pub banner_message: Option<String>,
    pub code_preverified: bool,
}

impl Default for PasswordResetFlowState {
    fn default() -> Self {
        Self {
            active: false,
            identifier: String::new(),
            recovery_code: String::new(),
            show_recovery_code: true,
            new_password: String::new(),
            confirm_password: String::new(),
            show_new_password: false,
            busy: false,
            banner_message: None,
            code_preverified: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ServerProbeState {
    #[default]
    Untested,
    Checking {
        server_url: String,
    },
    Connected {
        server_url: String,
        version: String,
        registration_open: bool,
    },
    Failed {
        server_url: String,
        message: String,
    },
}

impl ServerProbeState {
    fn target_url(&self) -> Option<&str> {
        match self {
            Self::Untested => None,
            Self::Checking { server_url }
            | Self::Connected { server_url, .. }
            | Self::Failed { server_url, .. } => Some(server_url.as_str()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccountDraftState {
    pub tab: AuthTab,
    pub profile_section: ProfileSection,
    pub signed_out_section: SignedOutProfileSection,
    pub server_url: String,
    pub email_or_username: String,
    pub signup_username: String,
    pub display_name: String,
    pub password: String,
    pub show_password: bool,
    pub auth_busy: bool,
    pub password_reset: PasswordResetFlowState,
    pub probe: ServerProbeState,
    pub pending_conflict: Option<SyncConflictPreview>,
    pub conflict_resolving: bool,
    pub logout_confirm_open: bool,
    pub sign_out_after_sync: bool,
    pub profiles_panel_open: bool,
    pub new_profile_name: String,
    pub renaming_profile_id: Option<String>,
    pub rename_profile_draft: String,
    pub import_local_open: bool,
    pub edit_display_name: String,
    pub edit_username: String,
    pub profile_save_busy: bool,
    pub change_email_open: bool,
    pub new_email_draft: String,
    pub change_email_password: String,
    pub show_change_email_password: bool,
    pub change_email_busy: bool,
    pub current_password_draft: String,
    pub new_password_draft: String,
    pub confirm_new_password_draft: String,
    pub show_change_pw_current: bool,
    pub show_change_pw_new: bool,
    pub change_password_busy: bool,
    pub status_message: Option<String>,
    pub error_message: Option<String>,
}

impl Default for AccountDraftState {
    fn default() -> Self {
        Self {
            tab: AuthTab::SignIn,
            profile_section: ProfileSection::Profile,
            signed_out_section: SignedOutProfileSection::CloudAccount,
            server_url: DEFAULT_SERVER_URL.to_string(),
            email_or_username: String::new(),
            signup_username: String::new(),
            display_name: String::new(),
            password: String::new(),
            show_password: false,
            auth_busy: false,
            password_reset: PasswordResetFlowState::default(),
            probe: ServerProbeState::Untested,
            pending_conflict: None,
            conflict_resolving: false,
            logout_confirm_open: false,
            sign_out_after_sync: false,
            profiles_panel_open: false,
            new_profile_name: String::new(),
            renaming_profile_id: None,
            rename_profile_draft: String::new(),
            import_local_open: false,
            edit_display_name: String::new(),
            edit_username: String::new(),
            profile_save_busy: false,
            change_email_open: false,
            new_email_draft: String::new(),
            change_email_password: String::new(),
            show_change_email_password: false,
            change_email_busy: false,
            current_password_draft: String::new(),
            new_password_draft: String::new(),
            confirm_new_password_draft: String::new(),
            show_change_pw_current: false,
            show_change_pw_new: false,
            change_password_busy: false,
            status_message: None,
            error_message: None,
        }
    }
}

impl AccountDraftState {
    /// Returns true while a server health check is actively running.
    pub fn is_testing(&self) -> bool {
        matches!(self.probe, ServerProbeState::Checking { .. })
    }

    /// Resets any cached server probe when the draft URL no longer matches the probed target.
    pub fn invalidate_probe_if_url_changed(&mut self) {
        if let Some(probed) = self.probe.target_url()
            && !urls_match(&self.server_url, probed)
        {
            self.probe = ServerProbeState::Untested;
        }
    }

    /// Returns true when the active server URL was verified and explicitly closed registration.
    pub fn is_registration_closed(&self) -> bool {
        matches!(
            &self.probe,
            ServerProbeState::Connected {
                server_url,
                registration_open: false,
                ..
            } if urls_match(&self.server_url, server_url)
        )
    }

    /// Records a successful server health check if it still matches the current draft URL.
    pub fn apply_health_ok(&mut self, server_url: String, info: ServerInfoResponse) {
        if !urls_match(&self.server_url, &server_url) {
            return;
        }
        self.server_url = server_url.clone();
        self.probe = ServerProbeState::Connected {
            server_url,
            version: info.version,
            registration_open: info.registration_open,
        };
        self.error_message = None;
    }

    /// Records a failed server health check if it still matches the current draft URL.
    pub fn apply_health_err(&mut self, server_url: String, message: String) {
        if !urls_match(&self.server_url, &server_url) {
            return;
        }
        self.probe = ServerProbeState::Failed {
            server_url,
            message,
        };
    }

    /// Applies successful login/registration feedback to the draft state.
    pub fn apply_auth_ok(&mut self, server_url: &str, label: &str) {
        self.auth_busy = false;
        self.password.clear();
        self.signup_username.clear();
        self.password_reset = PasswordResetFlowState::default();
        self.pending_conflict = None;
        self.conflict_resolving = false;
        self.logout_confirm_open = false;
        self.sign_out_after_sync = false;
        self.profile_save_busy = false;
        self.change_password_busy = false;
        self.current_password_draft.clear();
        self.new_password_draft.clear();
        self.confirm_new_password_draft.clear();
        self.error_message = None;
        self.server_url = server_url.to_string();
        self.status_message = Some(format!("Signed in as {label}. Syncing notebooks..."));
    }

    /// Applies an authentication error message to the draft state.
    pub fn apply_auth_err(&mut self, message: String) {
        self.auth_busy = false;
        self.password_reset.busy = false;
        self.status_message = None;
        self.error_message = Some(message);
    }

    /// Transitions into the forced recovery-code password reset flow.
    pub fn enter_password_reset_flow(
        &mut self,
        identifier: String,
        prefill_code: Option<String>,
        message: String,
    ) {
        self.auth_busy = false;
        self.password.clear();
        self.error_message = None;
        self.status_message = None;
        self.password_reset.active = true;
        self.password_reset.identifier = identifier;
        self.password_reset.code_preverified = prefill_code.is_some();
        self.password_reset.recovery_code = prefill_code.unwrap_or_default();
        self.password_reset.show_recovery_code = true;
        self.password_reset.new_password.clear();
        self.password_reset.confirm_password.clear();
        self.password_reset.busy = false;
        self.password_reset.banner_message = Some(message);
    }

    /// Exits the recovery-code password reset flow back to normal sign-in.
    pub fn exit_password_reset_flow(&mut self) {
        self.password_reset = PasswordResetFlowState::default();
        self.error_message = None;
    }

    /// Syncs editable identity fields from an active cloud session.
    pub fn sync_identity_from_session(
        &mut self,
        session: Option<&drawcraft_sync::ActiveAccountSession>,
    ) {
        if let Some(sess) = session {
            self.edit_display_name = sess.user.display_name.clone();
            self.edit_username = sess.user.username.clone();
        } else {
            self.edit_display_name.clear();
            self.edit_username.clear();
        }
    }

    /// Applies successful profile update confirmation.
    pub fn apply_profile_updated(&mut self, username: &str, display_name: &str) {
        self.profile_save_busy = false;
        self.edit_username = username.to_string();
        self.edit_display_name = display_name.to_string();
        self.error_message = None;
        self.status_message = Some(format!("Profile updated (@{username})."));
    }

    /// Applies a profile update error message.
    pub fn apply_profile_update_err(&mut self, message: String) {
        self.profile_save_busy = false;
        self.status_message = None;
        self.error_message = Some(message);
    }

    /// Applies successful voluntary password change confirmation.
    pub fn apply_password_changed(&mut self) {
        self.change_password_busy = false;
        self.current_password_draft.clear();
        self.new_password_draft.clear();
        self.confirm_new_password_draft.clear();
        self.error_message = None;
        self.status_message = Some("Account password updated.".to_string());
    }

    /// Applies a password change error message.
    pub fn apply_password_change_err(&mut self, message: String) {
        self.change_password_busy = false;
        self.status_message = None;
        self.error_message = Some(message);
    }

    /// Applies a successful email change confirmation.
    pub fn apply_email_changed(&mut self, new_email: Option<&str>) {
        self.change_email_busy = false;
        self.change_email_open = false;
        self.new_email_draft.clear();
        self.change_email_password.clear();
        self.error_message = None;
        let label = new_email.unwrap_or("updated email");
        self.status_message = Some(format!("Account email updated to {label}."));
    }

    /// Applies an email change error message.
    pub fn apply_email_change_err(&mut self, message: String) {
        self.change_email_busy = false;
        self.status_message = None;
        self.error_message = Some(message);
    }

    /// Clears transient post-login sync banners once the initial sync settles.
    pub fn clear_transient_auth_status(&mut self) {
        if self
            .status_message
            .as_deref()
            .is_some_and(|msg| msg.starts_with("Signed in as "))
        {
            self.status_message = None;
        }
    }

    /// Resets transient authentication/confirmation flags after signing out to a local profile.
    pub fn reset_after_sign_out(&mut self, server_url: &str) {
        self.probe = ServerProbeState::Untested;
        self.pending_conflict = None;
        self.conflict_resolving = false;
        self.logout_confirm_open = false;
        self.sign_out_after_sync = false;
        self.password_reset = PasswordResetFlowState::default();
        self.signup_username.clear();
        self.edit_display_name.clear();
        self.edit_username.clear();
        self.profile_save_busy = false;
        self.change_email_open = false;
        self.change_email_password.clear();
        self.new_email_draft.clear();
        self.change_password_busy = false;
        self.current_password_draft.clear();
        self.new_password_draft.clear();
        self.confirm_new_password_draft.clear();
        self.server_url = server_url.to_string();
        self.error_message = None;
        self.status_message = Some(
            "Signed out. Switched back to your isolated local profile on this device.".to_string(),
        );
    }
}

pub(crate) fn sync_draft_from_session(app: &mut DrawCraftApp) {
    app.view.account.server_url = app.session.server_url().to_string();
    app.view
        .account
        .sync_identity_from_session(app.session.active_account_session());
    app.view.account.error_message = None;
    app.view.account.status_message = None;
    app.view.account.logout_confirm_open = false;
    app.view.account.sign_out_after_sync = false;
    app.view.account.profile_save_busy = false;
    app.view.account.change_email_open = false;
    app.view.account.change_email_password.clear();
    app.view.account.change_password_busy = false;
    app.view.account.current_password_draft.clear();
    app.view.account.new_password_draft.clear();
    app.view.account.confirm_new_password_draft.clear();
    app.view.account.invalidate_probe_if_url_changed();
}

pub(crate) fn render_body(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let mut should_close = false;
    if app.view.account.logout_confirm_open && app.session.is_signed_in() {
        should_close = render_logout_safeguard_body(app, ui, t);
    } else if app.session.is_signed_in() {
        crate::modal::apply_subflow_motion(
            app,
            ui,
            crate::modal::ActiveModal::AccountSync,
            3,
            3,
            |app, ui| {
                should_close = render_signed_in_body(app, ui, t);
            },
        );
    } else {
        should_close = render_signed_out_body(app, ui, t);
    }
    should_close
}

pub(crate) fn render_footer(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    if app.view.account.logout_confirm_open && app.session.is_signed_in() {
        render_logout_safeguard_footer(app, ui, t)
    } else if app.session.is_signed_in() {
        render_signed_in_footer(app, ui, t)
    } else {
        render_signed_out_footer(app, ui, t)
    }
}

pub(crate) fn render_first_access_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let mut submitted = false;
    ui.spacing_mut().item_spacing.y = 0.0;

    draw_section_card(ui, t, |ui| {
        draw_field_label_row(
            ui,
            "Nickname",
            FieldHint::Muted("Used only to identify this local workspace"),
            t,
        );
        ui.add_space(6.0);
        let resp = draw_text_input(
            ui,
            InputFieldSpec {
                id_salt: "first_access_nickname_input",
                width: ui.available_width(),
                hint: "e.g. Jordan or Ada",
                border_color: None,
                trailing: InputTrailing::None,
            },
            &mut app.view.first_access_nickname,
            t,
        );
        if app.view.first_access_focus_pending {
            ui.memory_mut(|m| {
                m.request_focus(ui.make_persistent_id("first_access_nickname_input"))
            });
            app.view.first_access_focus_pending = false;
        }
        if resp.changed {
            app.view.first_access_error = None;
        }
        if resp.enter_pressed {
            submitted = true;
        }
    });

    if let Some(ref err) = app.view.first_access_error {
        ui.add_space(8.0);
        draw_status_banner(ui, BannerTone::Error, err, t);
    }

    if submitted {
        return submit_first_access_nickname(app, ui.ctx());
    }
    false
}

pub(crate) fn render_first_access_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let mut should_close = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Required on first launch")
                .size(11.0)
                .color(t.text_muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let valid = !app.view.first_access_nickname.trim().is_empty();
            if primary_action_button(ui, "Start Session", valid, t).clicked() {
                should_close = submit_first_access_nickname(app, ui.ctx());
            }
        });
    });
    should_close
}

fn submit_first_access_nickname(app: &mut DrawCraftApp, ctx: &egui::Context) -> bool {
    let nickname = app.view.first_access_nickname.trim().to_string();
    match app.session.execute(
        "profile.completeFirstAccess",
        &json!({ "nickname": nickname }),
    ) {
        Ok(_) => {
            app.view.first_access_nickname.clear();
            app.view.first_access_error = None;
            crate::sync_view_to_active_notebook(app);
            crate::sync_worker::maybe_report_local_discovery(
                app,
                ctx,
                ctx.input(|i| i.time * 1000.0),
            );
            true
        }
        Err(err) => {
            app.view.first_access_error = Some(err.to_string());
            false
        }
    }
}

pub(crate) fn render_conflict_body(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let Some(preview) = app.view.account.pending_conflict.clone() else {
        return true;
    };
    let busy = app.view.account.conflict_resolving;
    let mut pick_server = false;
    let mut pick_local = false;

    ui.spacing_mut().item_spacing.y = 0.0;
    draw_section_card(ui, t, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "Your '{}' account ({}) has offline edits or diverged versions. Choose which version should prevail—the version not chosen will be permanently discarded.",
                    preview.account_name,
                    if preview.account_email.is_empty() {
                        "cloud account"
                    } else {
                        preview.account_email.as_str()
                    }
                ))
                .size(11.5)
                .color(t.text_muted),
            )
            .wrap(),
        );
    });

    ui.add_space(10.0);

    draw_section_card(ui, t, |ui| {
        ui.horizontal(|ui| {
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            paint_icon(ui.painter(), ir, Icon::Server, t.accent);
            ui.label(
                egui::RichText::new("Server Version")
                    .size(13.0)
                    .strong()
                    .color(t.text),
            );
        });
        ui.add_space(4.0);
        let sample = if preview.server_notebook_titles.is_empty() {
            String::new()
        } else {
            format!(" ({})", preview.server_notebook_titles.join(", "))
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} notebook(s) currently stored on server{sample}. Choosing this deletes conflicting local changes on this computer.",
                    preview.server_notebook_count
                ))
                .size(11.0)
                .color(t.text_muted),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        if secondary_action_button(ui, "Keep Server Version (Discard Local)", !busy, t).clicked() {
            pick_server = true;
        }
    });

    ui.add_space(9.0);

    draw_section_card(ui, t, |ui| {
        ui.horizontal(|ui| {
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            paint_icon(ui.painter(), ir, Icon::Inbox, t.accent);
            ui.label(
                egui::RichText::new("Local Version (from this computer)")
                    .size(13.0)
                    .strong()
                    .color(t.text),
            );
        });
        ui.add_space(4.0);
        let sample = if preview.local_dirty_titles.is_empty() {
            String::new()
        } else {
            format!(" ({})", preview.local_dirty_titles.join(", "))
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{} local notebook(s), {} pending change(s){sample}. Choosing this overwrites the server and deletes differing server data.",
                    preview.local_notebook_count, preview.local_pending_changes
                ))
                .size(11.0)
                .color(t.text_muted),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        if primary_action_button(ui, "Keep Local Version (Overwrite Server)", !busy, t).clicked() {
            pick_local = true;
        }
    });

    if let Some(ref err) = app.view.account.error_message {
        ui.add_space(8.0);
        draw_status_banner(ui, BannerTone::Error, err, t);
    }
    if busy {
        ui.add_space(8.0);
        draw_status_banner(
            ui,
            BannerTone::Info,
            "Applying your choice and discarding the other version...",
            t,
        );
    }

    if pick_server {
        trigger_conflict_keep_server(app, ui.ctx());
    } else if pick_local {
        trigger_conflict_keep_local(app, ui.ctx());
    }
    false
}

pub(crate) fn render_conflict_footer(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let mut should_close = false;
    let busy = app.view.account.conflict_resolving;
    ui.horizontal(|ui| {
        if secondary_action_button(ui, "Decide Later (Stay Offline)", !busy, t).clicked() {
            app.session.record_sync_failure(
                true,
                "Sync paused until Server vs Local conflict is resolved.".to_string(),
            );
            should_close = true;
        }
    });
    should_close
}

fn summary_account_label(summary: &SyncStatusSummary) -> String {
    summary
        .email
        .clone()
        .or_else(|| summary.username.as_ref().map(|u| format!("@{u}")))
        .unwrap_or_else(|| "your account".to_string())
}

pub(crate) fn render_logout_safeguard_body(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    ui.spacing_mut().item_spacing.y = 0.0;
    let pending = app.session.cloud_pending_changes_count();
    let summary = app.session.sync_status_summary();
    let acct = summary_account_label(&summary);

    draw_section_card(ui, t, |ui| {
        ui.horizontal(|ui| {
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            paint_icon(ui.painter(), ir, Icon::AlertCircle, t.warning_fg);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Unsynced Cloud Changes")
                    .size(14.0)
                    .strong()
                    .color(t.text),
            );
        });
        ui.add_space(6.0);
        let noun = if pending == 1 { "change" } else { "changes" };
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "{pending} {noun} in {acct} are still Waiting to sync (pending upload). Signing out before uploading means other devices will not see those edits yet."
                ))
                .size(11.5)
                .color(t.text_muted),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        draw_status_banner(
            ui,
            BannerTone::Warning,
            "Choose \"Sync & Sign Out\" to upload now, or \"Sign Out Anyway\" to return to your Local Only workspace immediately.",
            t,
        );
    });

    render_auth_banners(app, ui, t);
    false
}

pub(crate) fn render_logout_safeguard_actions(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) {
    let syncing = app.session.is_sync_in_flight() || app.view.account.sign_out_after_sync;
    ui.horizontal(|ui| {
        if secondary_action_button(ui, "Cancel", !syncing, t).clicked() {
            app.view.account.logout_confirm_open = false;
            app.view.account.sign_out_after_sync = false;
            app.view.account.error_message = None;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let sync_lbl = if syncing {
                "Syncing..."
            } else {
                "Sync & Sign Out"
            };
            if primary_action_button(ui, sync_lbl, !syncing, t).clicked() {
                app.view.account.error_message = None;
                app.view.account.status_message =
                    Some("Uploading pending changes before sign-out...".to_string());
                if !app.session.is_viewing_cloud() {
                    let _ = app.session.switch_to_cloud_workspace();
                }
                app.view.account.sign_out_after_sync = true;
                trigger_manual_sync(app, ui.ctx());
            }
            if danger_action_button(ui, "Sign Out Anyway", !syncing, t).clicked() {
                complete_sign_out(app, ui.ctx());
            }
        });
    });
}

fn render_logout_safeguard_footer(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    render_logout_safeguard_actions(app, ui, t);
    false
}

fn render_signed_out_body(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut submit_on_enter = false;
    let mut test_on_enter = false;

    draw_section_card(ui, t, |ui| {
        render_signed_out_local_workspace_card(app, ui, t);
    });

    ui.add_space(9.0);

    draw_section_card(ui, t, |ui| {
        test_on_enter = render_server_probe_section(app, ui, t, true);
    });

    ui.add_space(9.0);

    draw_section_card(ui, t, |ui| {
        if render_auth_card_contents(app, ui, t, false) {
            submit_on_enter = true;
        }
    });

    render_auth_banners(app, ui, t);

    if test_on_enter && !app.view.account.is_testing() {
        trigger_test_connection(app, ui.ctx());
    }
    if submit_on_enter {
        handle_auth_submit_on_enter(app, ui.ctx());
    }
    false
}

pub(crate) fn handle_auth_submit_on_enter(app: &mut DrawCraftApp, ctx: &egui::Context) {
    if app.view.account.password_reset.active {
        if !app.view.account.password_reset.busy {
            submit_complete_password_reset(app, ctx);
        }
    } else if !app.view.account.auth_busy
        && !(app.view.account.tab == AuthTab::CreateAccount
            && app.view.account.is_registration_closed())
    {
        submit_auth_request(app, ctx);
    }
}

pub(crate) fn render_auth_card_contents(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    show_inline_submit: bool,
) -> bool {
    if app.view.account.password_reset.active {
        return render_password_reset_flow_card(app, ui, t, show_inline_submit);
    }

    let mut submit_on_enter = false;
    draw_auth_mode_tabs(app, ui, t);
    ui.add_space(9.0);

    let tab = app.view.account.tab;
    let subflow_id = match tab {
        AuthTab::SignIn => 0,
        AuthTab::CreateAccount => 1,
    };
    crate::modal::apply_subflow_motion(
        app,
        ui,
        crate::modal::ActiveModal::AccountSync,
        subflow_id,
        isize::from(subflow_id),
        |app, ui| {
            render_auth_tab_explanation(app, ui, t);
            ui.add_space(8.0);

            if matches!(tab, AuthTab::CreateAccount) && app.view.account.is_registration_closed() {
                draw_status_banner(
                    ui,
                    BannerTone::Warning,
                    "Registration is closed on this server. Switch to Sign In to use an existing account.",
                    t,
                );
                ui.add_space(8.0);
            }

            if render_account_credentials_fields(app, ui, t) {
                submit_on_enter = true;
            }
        },
    );

    if tab == AuthTab::SignIn {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let link = ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Have a recovery code from the server?")
                            .size(11.0)
                            .color(t.accent),
                    )
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::ZERO),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if link.clicked() {
                let id = app.view.account.email_or_username.trim().to_string();
                app.view.account.enter_password_reset_flow(
                    id,
                    None,
                    "Enter the recovery code generated on the server and choose a new password."
                        .to_string(),
                );
            }
        });
    }

    if show_inline_submit {
        ui.add_space(10.0);
        let busy = app.view.account.auth_busy;
        let blocked_closed =
            tab == AuthTab::CreateAccount && app.view.account.is_registration_closed();
        let cta = match (tab, busy) {
            (AuthTab::SignIn, false) => "Sign In",
            (AuthTab::SignIn, true) => "Signing In...",
            (AuthTab::CreateAccount, false) => "Create Account",
            (AuthTab::CreateAccount, true) => "Creating...",
        };
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if primary_action_button(ui, cta, !busy && !blocked_closed, t).clicked() {
                    submit_auth_request(app, ui.ctx());
                }
            });
        });
    }

    submit_on_enter
}

fn render_password_reset_flow_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    show_inline_submit: bool,
) -> bool {
    let mut submit_on_enter = false;
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Reset, t.accent);
        ui.label(
            egui::RichText::new("Complete Password Reset")
                .size(13.5)
                .strong()
                .color(t.text),
        );
    });
    ui.add_space(4.0);
    let banner = app
        .view
        .account
        .password_reset
        .banner_message
        .clone()
        .unwrap_or_else(|| {
            "Enter the temporary recovery code generated on your server and choose a new password."
                .to_string()
        });
    draw_status_banner(
        ui,
        if app.view.account.password_reset.code_preverified {
            BannerTone::Info
        } else {
            BannerTone::Warning
        },
        &banner,
        t,
    );

    ui.add_space(8.0);
    draw_field_label_row(
        ui,
        "Email or Username",
        FieldHint::Muted("Account being recovered"),
        t,
    );
    ui.add_space(3.0);
    if draw_text_input(
        ui,
        InputFieldSpec {
            id_salt: "acct_reset_identifier",
            width: ui.available_width(),
            hint: "you@example.com or alex_notes",
            border_color: None,
            trailing: InputTrailing::None,
        },
        &mut app.view.account.password_reset.identifier,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    ui.add_space(7.0);
    let code_hint = if app.view.account.password_reset.code_preverified {
        FieldHint::Success("Recovery code verified")
    } else {
        FieldHint::Muted("Code generated on server")
    };
    draw_field_label_row(ui, "Temporary Recovery Code", code_hint, t);
    ui.add_space(3.0);
    if draw_password_input(
        ui,
        "acct_reset_code",
        ui.available_width(),
        &mut app.view.account.password_reset.recovery_code,
        &mut app.view.account.password_reset.show_recovery_code,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    ui.add_space(7.0);
    let pw_chars = app.view.account.password_reset.new_password.chars().count();
    let pw_hint = if pw_chars == 0 {
        FieldHint::Muted("Minimum 8 characters")
    } else if pw_chars >= MIN_PASSWORD_LEN {
        FieldHint::Success("Ready (8+ characters)")
    } else {
        FieldHint::Warning(format!("{pw_chars}/{MIN_PASSWORD_LEN} characters"))
    };
    draw_field_label_row(ui, "New Password", pw_hint, t);
    ui.add_space(3.0);
    if draw_password_input(
        ui,
        "acct_reset_new_pw",
        ui.available_width(),
        &mut app.view.account.password_reset.new_password,
        &mut app.view.account.password_reset.show_new_password,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    ui.add_space(7.0);
    let confirm_hint = if app.view.account.password_reset.confirm_password.is_empty() {
        FieldHint::Muted("Re-enter new password")
    } else if app.view.account.password_reset.new_password
        == app.view.account.password_reset.confirm_password
    {
        FieldHint::Success("Passwords match")
    } else {
        FieldHint::Warning("Passwords do not match".to_string())
    };
    draw_field_label_row(ui, "Confirm New Password", confirm_hint, t);
    ui.add_space(3.0);
    if draw_password_input(
        ui,
        "acct_reset_confirm_pw",
        ui.available_width(),
        &mut app.view.account.password_reset.confirm_password,
        &mut app.view.account.password_reset.show_new_password,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    if show_inline_submit {
        ui.add_space(10.0);
        let busy = app.view.account.password_reset.busy;
        ui.horizontal(|ui| {
            if secondary_action_button(ui, "Back to Sign In", !busy, t).clicked() {
                app.view.account.exit_password_reset_flow();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cta = if busy {
                    "Updating Password..."
                } else {
                    "Set New Password & Sign In"
                };
                if primary_action_button(ui, cta, !busy, t).clicked() {
                    submit_complete_password_reset(app, ui.ctx());
                }
            });
        });
    }

    submit_on_enter
}

fn render_signed_out_local_workspace_card(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let active_name = app
        .session
        .active_local_profile()
        .map_or_else(|| "Personal".to_string(), |p| p.nickname.clone());
    let nb_count = app.session.notebooks.len();
    let profile_count = app.session.accessible_local_profiles().len();

    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Inbox, t.accent);
        ui.label(
            egui::RichText::new(format!("On This Device: {active_name}"))
                .size(13.0)
                .strong()
                .color(t.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let toggle_lbl = if app.view.account.profiles_panel_open {
                "Hide Profiles"
            } else {
                "Profiles"
            };
            if fixed_secondary_button(ui, toggle_lbl, 86.0, true, t)
                .on_hover_text(format!("Manage {profile_count} offline profile(s)"))
                .clicked()
            {
                app.view.account.profiles_panel_open = !app.view.account.profiles_panel_open;
            }
            draw_pill_badge(
                ui,
                PillStyle {
                    fill: t.toolbar_bg,
                    border: t.border,
                    fg: t.text_muted,
                    icon: PillIcon::Dot,
                },
                "Local Only",
            );
        });
    });
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new(format!(
            "{nb_count} notebook(s) in this local workspace. Sync options become available after signing in to an account."
        ))
        .size(11.0)
        .color(t.text_muted),
    );

    if app.view.account.profiles_panel_open {
        ui.add_space(8.0);
        draw_hairline(ui, t);
        ui.add_space(8.0);
        render_local_profiles_manager(app, ui, t);
    }
}

fn render_auth_tab_explanation(app: &DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let msg = match app.view.account.tab {
        AuthTab::SignIn => {
            "Sign in to an existing cloud account to synchronize your notebooks across devices."
        }
        AuthTab::CreateAccount => {
            "Create a new synced account identified by a unique email. You can choose whether to bring in local accounts after signing in."
        }
    };
    ui.add(egui::Label::new(egui::RichText::new(msg).size(11.0).color(t.text_muted)).wrap());
}

fn render_signup_username_column(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let uname_trimmed = app.view.account.signup_username.trim();
    let uname_hint = if uname_trimmed.is_empty() {
        FieldHint::Muted("Optional")
    } else {
        match normalize_username(uname_trimmed) {
            Ok(_) => FieldHint::Success("Valid @username"),
            Err(err) => FieldHint::Warning(err.to_string()),
        }
    };
    draw_field_label_row(ui, "Username", uname_hint, t);
    ui.add_space(3.0);
    draw_text_input(
        ui,
        InputFieldSpec {
            id_salt: "acct_signup_username",
            width: ui.available_width(),
            hint: "alex_notes (auto if empty)",
            border_color: None,
            trailing: InputTrailing::None,
        },
        &mut app.view.account.signup_username,
        t,
    )
    .enter_pressed
}

fn render_signup_display_name_column(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    draw_field_label_row(ui, "Display Name", FieldHint::Muted("Optional"), t);
    ui.add_space(3.0);
    draw_text_input(
        ui,
        InputFieldSpec {
            id_salt: "acct_display_name",
            width: ui.available_width(),
            hint: "Shown across devices",
            border_color: None,
            trailing: InputTrailing::None,
        },
        &mut app.view.account.display_name,
        t,
    )
    .enter_pressed
}

fn render_account_credentials_fields(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> bool {
    let mut submit_on_enter = false;
    let tab = app.view.account.tab;
    let is_email_only = matches!(tab, AuthTab::CreateAccount);

    let (id_label, id_hint, id_placeholder) = if is_email_only {
        let hint = if app.view.account.email_or_username.trim().is_empty() {
            FieldHint::Muted("Unique account email")
        } else if normalize_email(&app.view.account.email_or_username).is_ok() {
            FieldHint::Success("Valid email")
        } else {
            FieldHint::Warning("Enter a valid email address".to_string())
        };
        ("Email", hint, "you@example.com")
    } else {
        (
            "Email or Username",
            FieldHint::Muted("Account identifier"),
            "you@example.com or alex_notes",
        )
    };

    draw_field_label_row(ui, id_label, id_hint, t);
    ui.add_space(3.0);

    let user_trailing = if tab == AuthTab::CreateAccount
        && normalize_email(&app.view.account.email_or_username).is_ok()
    {
        InputTrailing::GreenCheck("Valid email format")
    } else {
        InputTrailing::None
    };
    let resp = draw_text_input(
        ui,
        InputFieldSpec {
            id_salt: "acct_email_or_username",
            width: ui.available_width(),
            hint: id_placeholder,
            border_color: None,
            trailing: user_trailing,
        },
        &mut app.view.account.email_or_username,
        t,
    );
    if resp.enter_pressed {
        submit_on_enter = true;
    }

    if is_email_only {
        ui.add_space(7.0);
        let avail_w = ui.available_width();
        if avail_w >= 360.0 {
            let col_gap = 10.0_f32;
            let col_w = ((avail_w - col_gap) * 0.5).max(120.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = col_gap;
                ui.allocate_ui_with_layout(
                    Vec2::new(col_w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(col_w);
                        if render_signup_username_column(app, ui, t) {
                            submit_on_enter = true;
                        }
                    },
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(col_w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(col_w);
                        if render_signup_display_name_column(app, ui, t) {
                            submit_on_enter = true;
                        }
                    },
                );
            });
        } else {
            if render_signup_username_column(app, ui, t) {
                submit_on_enter = true;
            }
            ui.add_space(7.0);
            if render_signup_display_name_column(app, ui, t) {
                submit_on_enter = true;
            }
        }
    }

    ui.add_space(7.0);
    let pw_chars = app.view.account.password.chars().count();
    let pw_hint = if tab == AuthTab::CreateAccount && pw_chars > 0 {
        if pw_chars >= MIN_PASSWORD_LEN {
            FieldHint::Success("Ready (8+ characters)")
        } else {
            FieldHint::Warning(format!("{pw_chars}/{MIN_PASSWORD_LEN} characters"))
        }
    } else {
        FieldHint::Muted("Minimum 8 characters")
    };
    draw_field_label_row(ui, "Password", pw_hint, t);
    ui.add_space(3.0);
    let pw_resp = draw_password_input(
        ui,
        "acct_password",
        ui.available_width(),
        &mut app.view.account.password,
        &mut app.view.account.show_password,
        t,
    );
    if pw_resp.enter_pressed {
        submit_on_enter = true;
    }

    submit_on_enter
}

pub(crate) fn render_server_probe_section(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    show_examples: bool,
) -> bool {
    let mut test_on_enter = false;
    let probe = app.view.account.probe.clone();

    ui.horizontal(|ui| {
        let icon_color = match probe {
            ServerProbeState::Connected { .. } => t.success_fg,
            _ => t.accent,
        };
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Server, icon_color);
        ui.label(
            egui::RichText::new("Self-Hosted Server")
                .size(13.0)
                .strong()
                .color(t.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_probe_status_pill(ui, &probe, t);
        });
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Connect to your local DrawCraft Server or mesh VPN tunnel address.")
            .size(11.0)
            .color(t.text_muted),
    );
    ui.add_space(8.0);

    let (border_override, trailing) = match &probe {
        ServerProbeState::Connected { .. } => (
            Some(t.success_border),
            InputTrailing::GreenCheck("Server connection verified"),
        ),
        ServerProbeState::Failed { .. } => (
            Some(t.danger_border),
            InputTrailing::Icon(Icon::AlertCircle, t.danger_fg, "Server connection failed"),
        ),
        ServerProbeState::Checking { .. } => (
            None,
            InputTrailing::Icon(Icon::Sync, t.accent, "Checking connection..."),
        ),
        ServerProbeState::Untested => (None, InputTrailing::None),
    };

    let testing = app.view.account.is_testing();
    let btn_w = 84.0_f32;
    let gap = 8.0_f32;
    let input_w = (ui.available_width() - btn_w - gap).max(140.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let url_resp = draw_text_input(
            ui,
            InputFieldSpec {
                id_salt: "acct_server_url_signed_out",
                width: input_w,
                hint: DEFAULT_SERVER_URL,
                border_color: border_override,
                trailing,
            },
            &mut app.view.account.server_url,
            t,
        );
        if url_resp.changed {
            app.view.account.invalidate_probe_if_url_changed();
            app.view.account.error_message = None;
        }
        if url_resp.enter_pressed {
            test_on_enter = true;
        }
        let test_lbl = if testing { "Checking" } else { "Test" };
        if fixed_secondary_button(ui, test_lbl, btn_w, !testing, t)
            .on_hover_text("Verify server reachability (Enter)")
            .clicked()
        {
            trigger_test_connection(app, ui.ctx());
        }
    });

    ui.add_space(7.0);
    render_probe_feedback_row(app, ui, &probe, show_examples, t);

    test_on_enter
}

fn render_probe_feedback_row(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    probe: &ServerProbeState,
    show_examples: bool,
    t: &Tokens,
) {
    match probe {
        ServerProbeState::Untested => {
            ui.horizontal(|ui| {
                let help = if show_examples {
                    "Local: http://127.0.0.1:8787  •  VPN: http://100.x.y.z:8787"
                } else {
                    "Press Enter or click Test to check server reachability."
                };
                ui.label(egui::RichText::new(help).size(10.5).color(t.text_muted));
                if app.view.account.server_url.trim() != DEFAULT_SERVER_URL {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let link = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Use localhost")
                                        .size(10.5)
                                        .color(t.accent),
                                )
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .min_size(Vec2::ZERO),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if link.clicked() {
                            app.view.account.server_url = DEFAULT_SERVER_URL.to_string();
                            app.view.account.invalidate_probe_if_url_changed();
                            app.view.account.error_message = None;
                        }
                    });
                }
            });
        }
        ServerProbeState::Checking { .. } => {
            draw_status_banner(
                ui,
                BannerTone::Info,
                "Checking connection to DrawCraft Server...",
                t,
            );
        }
        ServerProbeState::Connected {
            version,
            registration_open,
            ..
        } => {
            draw_connected_server_banner(ui, version, *registration_open, t);
        }
        ServerProbeState::Failed { message, .. } => {
            draw_status_banner(ui, BannerTone::Error, message, t);
        }
    }
}

fn draw_connected_server_banner(
    ui: &mut egui::Ui,
    version: &str,
    registration_open: bool,
    t: &Tokens,
) {
    egui::Frame::new()
        .fill(t.success_bg)
        .stroke(Stroke::new(1.0, t.success_border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (badge_rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                paint_green_check_badge(ui.painter(), badge_rect, t);
                ui.label(
                    egui::RichText::new(format!("Connected to DrawCraft Server v{version}"))
                        .size(11.5)
                        .strong()
                        .color(t.success_fg),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (reg_lbl, reg_fg) = if registration_open {
                        ("Registration open", t.success_fg)
                    } else {
                        ("Registration closed", t.warning_fg)
                    };
                    ui.label(
                        egui::RichText::new(reg_lbl)
                            .size(10.5)
                            .strong()
                            .color(reg_fg),
                    );
                });
            });
        });
}

fn draw_probe_status_pill(ui: &mut egui::Ui, probe: &ServerProbeState, t: &Tokens) {
    match probe {
        ServerProbeState::Untested => {
            draw_pill_badge(
                ui,
                PillStyle {
                    fill: t.toolbar_bg,
                    border: t.border,
                    fg: t.text_muted,
                    icon: PillIcon::None,
                },
                "Not tested",
            );
        }
        ServerProbeState::Checking { .. } => {
            draw_pill_badge(
                ui,
                PillStyle {
                    fill: t.accent_soft,
                    border: t.accent,
                    fg: t.accent,
                    icon: PillIcon::Vector(Icon::Sync),
                },
                "Checking...",
            );
        }
        ServerProbeState::Connected { .. } => {
            draw_pill_badge(
                ui,
                PillStyle {
                    fill: t.success_bg,
                    border: t.success_border,
                    fg: t.success_fg,
                    icon: PillIcon::GreenCheck(t),
                },
                "Connected",
            );
        }
        ServerProbeState::Failed { .. } => {
            draw_pill_badge(
                ui,
                PillStyle {
                    fill: t.danger_bg,
                    border: t.danger_border,
                    fg: t.danger_fg,
                    icon: PillIcon::Vector(Icon::AlertCircle),
                },
                "Unreachable",
            );
        }
    }
}

fn render_signed_out_footer(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let mut should_close = false;
    ui.horizontal(|ui| {
        if app.view.account.password_reset.active {
            let busy = app.view.account.password_reset.busy;
            if secondary_action_button(ui, "Back to Sign In", !busy, t).clicked() {
                app.view.account.exit_password_reset_flow();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cta = if busy {
                    "Updating..."
                } else {
                    "Set New Password & Sign In"
                };
                if primary_action_button(ui, cta, !busy, t).clicked() {
                    submit_complete_password_reset(app, ui.ctx());
                }
            });
            return;
        }

        if secondary_action_button(ui, "Cancel", true, t).clicked() {
            should_close = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.view.account.auth_busy;
            let blocked_closed = app.view.account.tab == AuthTab::CreateAccount
                && app.view.account.is_registration_closed();
            let cta = match (app.view.account.tab, busy) {
                (AuthTab::SignIn, false) => "Sign In",
                (AuthTab::SignIn, true) => "Signing In...",
                (AuthTab::CreateAccount, false) => "Create Account",
                (AuthTab::CreateAccount, true) => "Creating...",
            };
            if primary_action_button(ui, cta, !busy && !blocked_closed, t).clicked() {
                submit_auth_request(app, ui.ctx());
            }
        });
    });
    should_close
}

fn render_signed_in_body(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    ui.spacing_mut().item_spacing.y = 0.0;
    let summary = app.session.sync_status_summary();
    let cloud_pending = app.session.cloud_pending_changes_count();

    draw_section_card(ui, t, |ui| {
        render_signed_in_account_card(app, ui, &summary, cloud_pending, t);
    });

    if !app.session.accessible_local_profiles().is_empty() {
        ui.add_space(10.0);
        draw_section_card(ui, t, |ui| {
            render_detected_local_accounts_migration_card(app, ui, &summary, t);
        });
    }

    ui.add_space(10.0);

    draw_section_card(ui, t, |ui| {
        render_workspaces_and_transfer_card(app, ui, &summary, t);
    });

    ui.add_space(10.0);

    let mut test_on_enter = false;
    draw_section_card(ui, t, |ui| {
        test_on_enter = render_signed_in_server_card(app, ui, &summary, t);
    });
    if test_on_enter && !app.view.account.is_testing() {
        trigger_test_connection(app, ui.ctx());
    }

    if let Some(ref conflict) = summary.last_conflict_notice {
        ui.add_space(8.0);
        draw_status_banner(ui, BannerTone::Info, conflict, t);
    }
    if let Some(ref err) = summary.last_error
        && app.view.account.error_message.is_none()
    {
        ui.add_space(8.0);
        draw_status_banner(ui, BannerTone::Error, err, t);
    }
    if summary.last_error.is_none() || app.view.account.error_message.is_some() {
        render_auth_banners(app, ui, t);
    }
    false
}

pub(crate) fn render_signed_in_account_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    cloud_pending: usize,
    t: &Tokens,
) {
    let display = summary
        .display_name
        .clone()
        .unwrap_or_else(|| "Account".to_string());
    let primary_id = if summary.signed_in {
        summary_account_label(summary)
    } else {
        "Cloud Account".to_string()
    };

    ui.horizontal(|ui| {
        let (avatar_rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
        ui.painter()
            .rect_filled(avatar_rect, CornerRadius::same(9), t.accent_soft);
        ui.painter().rect_stroke(
            avatar_rect,
            CornerRadius::same(9),
            Stroke::new(1.0, t.accent.gamma_multiply(0.35)),
            StrokeKind::Inside,
        );
        paint_icon(
            ui.painter(),
            avatar_rect.shrink(7.0),
            Icon::Account,
            t.accent,
        );

        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(&display)
                        .size(14.0)
                        .strong()
                        .color(t.text),
                );
                ui.label(
                    egui::RichText::new(&primary_id)
                        .size(11.5)
                        .color(t.text_muted),
                );
            });
            ui.label(
                egui::RichText::new(format!("Device: {}", summary.device_name))
                    .size(11.0)
                    .color(t.text_muted),
            );
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if summary.viewing_cloud {
                draw_sync_connection_pill(ui, summary.connection, t);
            } else {
                draw_pill_badge(
                    ui,
                    PillStyle {
                        fill: t.toolbar_bg,
                        border: t.border,
                        fg: t.text_muted,
                        icon: PillIcon::Dot,
                    },
                    "Viewing Local",
                );
            }
        });
    });

    if !summary.viewing_cloud {
        ui.add_space(8.0);
        let prof_name = summary
            .active_local_profile_name
            .as_deref()
            .unwrap_or("Local Profile");
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Active workspace is Local Only ({prof_name})."))
                    .size(11.0)
                    .color(t.warning_fg),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if fixed_secondary_button(ui, "Switch to Cloud", 118.0, true, t).clicked() {
                    crate::canvas::commit_pending_text(app);
                    if let Err(err) = app
                        .session
                        .execute("profile.switch", &json!({ "target": "cloud" }))
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
    }

    ui.add_space(9.0);
    draw_hairline(ui, t);
    ui.add_space(8.0);

    let queue_value = match cloud_pending {
        0 => "Up to date".to_string(),
        1 => "1 pending push".to_string(),
        n => format!("{n} pending push"),
    };
    let last_sync_value = format_last_sync(summary.last_sync_at_ms);
    let mode_value = if summary.viewing_cloud {
        "Cloud Sync".to_string()
    } else {
        summary
            .active_local_profile_name
            .clone()
            .map_or_else(|| "Local Only".to_string(), |n| format!("Local · {n}"))
    };

    ui.columns(3, |cols| {
        draw_metric_cell(&mut cols[0], "PENDING UPLOADS", &queue_value, t);
        draw_metric_cell(&mut cols[1], "LAST SYNC", &last_sync_value, t);
        draw_metric_cell(&mut cols[2], "WORKSPACE", &mode_value, t);
    });

    ui.add_space(8.0);
    render_account_email_row(app, ui, summary, t);
}

fn render_account_email_row(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) {
    ui.horizontal(|ui| {
        let email_text = match summary.email.as_deref() {
            Some(email) if !email.trim().is_empty() => format!("Account Email: {email}"),
            _ => "No email set — add one for cross-device sign-in".to_string(),
        };
        ui.label(
            egui::RichText::new(email_text)
                .size(11.0)
                .color(t.text_muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let toggle_lbl = if app.view.account.change_email_open {
                "Close"
            } else if summary.email.is_some() {
                "Change Email"
            } else {
                "Add Email"
            };
            if fixed_secondary_button(ui, toggle_lbl, 96.0, true, t).clicked() {
                app.view.account.change_email_open = !app.view.account.change_email_open;
                app.view.account.error_message = None;
            }
        });
    });

    if !app.view.account.change_email_open {
        return;
    }

    ui.add_space(8.0);
    draw_field_label_row(ui, "New Email", FieldHint::Muted("Must be unique"), t);
    ui.add_space(3.0);
    let mut submit_email = false;
    let email_resp = draw_text_input(
        ui,
        InputFieldSpec {
            id_salt: "acct_change_email_new",
            width: ui.available_width(),
            hint: "new@example.com",
            border_color: None,
            trailing: InputTrailing::None,
        },
        &mut app.view.account.new_email_draft,
        t,
    );
    if email_resp.enter_pressed {
        submit_email = true;
    }

    ui.add_space(6.0);
    draw_field_label_row(
        ui,
        "Current Password",
        FieldHint::Muted("Verify identity"),
        t,
    );
    ui.add_space(3.0);
    let pw_resp = draw_password_input(
        ui,
        "acct_change_email_pw",
        ui.available_width(),
        &mut app.view.account.change_email_password,
        &mut app.view.account.show_change_email_password,
        t,
    );
    if pw_resp.enter_pressed {
        submit_email = true;
    }

    ui.add_space(7.0);
    let busy = app.view.account.change_email_busy;
    ui.horizontal(|ui| {
        let save_lbl = if busy { "Saving..." } else { "Save Email" };
        if fixed_secondary_button(ui, save_lbl, 96.0, !busy, t).clicked() || submit_email {
            submit_change_email(app, ui.ctx());
        }
    });
}

pub(crate) fn render_profile_identity_card(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let Some(sess) = app.session.active_account_session().cloned() else {
        return;
    };
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Account, t.accent);
        ui.label(
            egui::RichText::new("Profile Details")
                .size(13.5)
                .strong()
                .color(t.text),
        );
    });
    ui.add_space(3.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "Update how your name appears across devices and change your unique @username handle used for signing in.",
            )
            .size(11.0)
            .color(t.text_muted),
        )
        .wrap(),
    );
    ui.add_space(10.0);

    let mut submit_on_enter = false;
    draw_field_label_row(
        ui,
        "Display Name",
        FieldHint::Muted("Shown on synced devices"),
        t,
    );
    ui.add_space(3.0);
    if draw_text_input(
        ui,
        InputFieldSpec {
            id_salt: "profile_edit_display_name",
            width: ui.available_width(),
            hint: "Your name",
            border_color: None,
            trailing: InputTrailing::None,
        },
        &mut app.view.account.edit_display_name,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    ui.add_space(8.0);
    let username_validation = normalize_username(&app.view.account.edit_username);
    let username_hint = match &username_validation {
        Ok((clean, _)) => {
            if *clean == sess.user.username {
                FieldHint::Muted("Current @username")
            } else {
                FieldHint::Success("Valid @username")
            }
        }
        Err(err) => FieldHint::Warning(err.to_string()),
    };
    draw_field_label_row(ui, "Username", username_hint, t);
    ui.add_space(3.0);
    let trailing = match &username_validation {
        Ok((clean, _)) if *clean != sess.user.username => {
            InputTrailing::GreenCheck("Valid username format")
        }
        _ => InputTrailing::None,
    };
    if draw_text_input(
        ui,
        InputFieldSpec {
            id_salt: "profile_edit_username",
            width: ui.available_width(),
            hint: "alex_notes",
            border_color: None,
            trailing,
        },
        &mut app.view.account.edit_username,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    let clean_display = sanitize_display_name(
        &app.view.account.edit_display_name,
        username_validation
            .as_ref()
            .map_or(sess.user.username.as_str(), |(u, _)| u.as_str()),
    );
    let username_changed = username_validation
        .as_ref()
        .is_ok_and(|(u, _)| *u != sess.user.username);
    let display_changed = clean_display != sess.user.display_name;
    let dirty = username_changed || display_changed;
    let valid = username_validation.is_ok();
    let busy = app.view.account.profile_save_busy;

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if secondary_action_button(ui, "Reset", dirty && !busy, t).clicked() {
            app.view.account.sync_identity_from_session(Some(&sess));
            app.view.account.error_message = None;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save_lbl = if busy { "Saving..." } else { "Save Profile" };
            if primary_action_button(ui, save_lbl, dirty && valid && !busy, t).clicked()
                || (submit_on_enter && dirty && valid && !busy)
            {
                submit_update_profile(app, ui.ctx());
            }
        });
    });
}

pub(crate) fn render_account_email_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) {
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Link, t.accent);
        ui.label(
            egui::RichText::new("Email Address")
                .size(13.5)
                .strong()
                .color(t.text),
        );
    });
    ui.add_space(3.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "Add or change the unique email linked to your account for signing in across devices.",
            )
            .size(11.0)
            .color(t.text_muted),
        )
        .wrap(),
    );
    ui.add_space(8.0);
    render_account_email_row(app, ui, summary, t);
}

pub(crate) fn render_change_password_card(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Reset, t.accent);
        ui.label(
            egui::RichText::new("Change Password")
                .size(13.5)
                .strong()
                .color(t.text),
        );
    });
    ui.add_space(3.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "Update your account password. If an administrator generated a recovery code on the server, use that code during sign-in instead.",
            )
            .size(11.0)
            .color(t.text_muted),
        )
        .wrap(),
    );
    ui.add_space(10.0);

    let mut submit_on_enter = false;
    draw_field_label_row(
        ui,
        "Current Password",
        FieldHint::Muted("Verify your identity"),
        t,
    );
    ui.add_space(3.0);
    if draw_password_input(
        ui,
        "profile_change_pw_current",
        ui.available_width(),
        &mut app.view.account.current_password_draft,
        &mut app.view.account.show_change_pw_current,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    ui.add_space(8.0);
    let new_chars = app.view.account.new_password_draft.chars().count();
    let new_hint = if new_chars == 0 {
        FieldHint::Muted("Minimum 8 characters")
    } else if new_chars >= MIN_PASSWORD_LEN {
        FieldHint::Success("Ready (8+ characters)")
    } else {
        FieldHint::Warning(format!("{new_chars}/{MIN_PASSWORD_LEN} characters"))
    };
    draw_field_label_row(ui, "New Password", new_hint, t);
    ui.add_space(3.0);
    if draw_password_input(
        ui,
        "profile_change_pw_new",
        ui.available_width(),
        &mut app.view.account.new_password_draft,
        &mut app.view.account.show_change_pw_new,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    ui.add_space(8.0);
    let confirm_hint = if app.view.account.confirm_new_password_draft.is_empty() {
        FieldHint::Muted("Re-enter new password")
    } else if app.view.account.new_password_draft == app.view.account.confirm_new_password_draft {
        FieldHint::Success("Passwords match")
    } else {
        FieldHint::Warning("Passwords do not match".to_string())
    };
    draw_field_label_row(ui, "Confirm New Password", confirm_hint, t);
    ui.add_space(3.0);
    if draw_password_input(
        ui,
        "profile_change_pw_confirm",
        ui.available_width(),
        &mut app.view.account.confirm_new_password_draft,
        &mut app.view.account.show_change_pw_new,
        t,
    )
    .enter_pressed
    {
        submit_on_enter = true;
    }

    let busy = app.view.account.change_password_busy;
    let ready = !app.view.account.current_password_draft.is_empty()
        && validate_password(&app.view.account.new_password_draft).is_ok()
        && app.view.account.new_password_draft == app.view.account.confirm_new_password_draft;

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let lbl = if busy {
                "Updating..."
            } else {
                "Update Password"
            };
            if primary_action_button(ui, lbl, ready && !busy, t).clicked()
                || (submit_on_enter && ready && !busy)
            {
                submit_change_password(app, ui.ctx());
            }
        });
    });
}

pub(crate) fn render_session_sign_out_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) {
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Server, t.accent);
        ui.label(
            egui::RichText::new("Session & Sign Out")
                .size(13.5)
                .strong()
                .color(t.text),
        );
    });
    ui.add_space(4.0);
    let user_id = app
        .session
        .active_account_session()
        .map_or_else(String::new, |s| s.user.user_id.clone());
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!(
                "Signed in on `{}` · Server {} · Account ID {}",
                summary.device_name, summary.server_url, user_id
            ))
            .size(11.0)
            .color(t.text_muted),
        )
        .wrap(),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if danger_action_button(ui, "Sign Out", true, t).clicked() {
            if app.session.cloud_pending_changes_count() > 0 {
                app.view.account.logout_confirm_open = true;
                app.view.account.error_message = None;
            } else {
                complete_sign_out(app, ui.ctx());
            }
        }
    });
}

pub(crate) fn render_detected_local_accounts_migration_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) {
    let profiles = app.session.accessible_local_profiles();
    let acct_name = summary
        .display_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(summary.username.as_deref())
        .unwrap_or("Cloud")
        .to_string();

    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Transfer, t.accent);
        ui.label(
            egui::RichText::new("Detected Local Accounts on This Device")
                .size(13.0)
                .strong()
                .color(t.text),
        );
    });
    ui.add_space(4.0);

    let mut migrate_target: Option<String> = None;
    for prof in profiles {
        let nb_count = app
            .session
            .list_local_profile_notebooks(&prof.local_profile_id)
            .len();
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(t.toolbar_bg)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "A local account '{}' was found ({} notebook(s)). Do you want to sync it with your '{}' account?",
                            prof.nickname, nb_count, acct_name
                        ))
                        .size(12.0)
                        .strong()
                        .color(t.text),
                    )
                    .wrap(),
                );
                ui.add_space(3.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "Confirming moves all data into your '{}' account, renames each notebook with ' - {}', and permanently deletes the old local account.",
                            acct_name, prof.nickname
                        ))
                        .size(11.0)
                        .color(t.text_muted),
                    )
                    .wrap(),
                );
                ui.add_space(6.0);
                if primary_action_button(
                    ui,
                    &format!("Sync '{}' into '{}'", prof.nickname, acct_name),
                    true,
                    t,
                )
                .clicked()
                {
                    migrate_target = Some(prof.local_profile_id);
                }
            });
    }

    if let Some(profile_id) = migrate_target {
        match app.session.execute(
            "sync.migrateLocalAccount",
            &json!({ "profile_id": profile_id }),
        ) {
            Ok(val) => {
                let src_nick = val["sourceNickname"]
                    .as_str()
                    .unwrap_or("Local account")
                    .to_string();
                let moved = val["movedNotebooks"].as_u64().unwrap_or(0);
                app.view.account.error_message = None;
                app.view.account.status_message = Some(format!(
                    "Migrated {moved} notebook(s) from '{src_nick}' into '{acct_name}' and deleted the old local account."
                ));
                crate::sync_view_to_active_notebook(app);
                crate::sync_worker::maybe_report_local_discovery(
                    app,
                    ui.ctx(),
                    ui.ctx().input(|i| i.time * 1000.0),
                );
                trigger_manual_sync(app, ui.ctx());
            }
            Err(err) => {
                app.view.account.status_message = None;
                app.view.account.error_message = Some(err.to_string());
            }
        }
    }
}

fn render_workspaces_and_transfer_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) {
    let profile_count = app.session.accessible_local_profiles().len();
    ui.horizontal(|ui| {
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Inbox, t.accent);
        ui.label(
            egui::RichText::new(format!("Workspaces & Local Profiles ({profile_count})"))
                .size(13.0)
                .strong()
                .color(t.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let lbl = if app.view.account.profiles_panel_open {
                "Collapse"
            } else {
                "Manage"
            };
            if fixed_secondary_button(ui, lbl, 80.0, true, t).clicked() {
                app.view.account.profiles_panel_open = !app.view.account.profiles_panel_open;
            }
        });
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(
            "Switch cleanly between Cloud Sync and isolated Local Only profiles, or transfer individual local notebooks to Cloud.",
        )
        .size(11.0)
        .color(t.text_muted),
    );

    if !app.view.account.profiles_panel_open {
        return;
    }

    ui.add_space(8.0);
    draw_hairline(ui, t);
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        let acct_label = if summary.signed_in {
            summary_account_label(summary)
        } else {
            "Cloud Account".to_string()
        };
        ui.label(
            egui::RichText::new(format!("Cloud Workspace · {acct_label}"))
                .size(12.0)
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
            } else if fixed_secondary_button(ui, "Switch", 70.0, true, t).clicked() {
                crate::canvas::commit_pending_text(app);
                if let Err(err) = app
                    .session
                    .execute("profile.switch", &json!({ "target": "cloud" }))
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

    ui.add_space(8.0);
    render_local_profiles_manager(app, ui, t);

    ui.add_space(8.0);
    draw_hairline(ui, t);
    ui.add_space(8.0);
    render_signed_in_transfer_section(app, ui, t);
}

pub(crate) fn render_local_profiles_manager(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let profiles = app.session.accessible_local_profiles();
    let active_local_id = app
        .session
        .active_local_profile()
        .map(|p| p.local_profile_id.clone())
        .unwrap_or_default();
    let viewing_cloud = app.session.is_viewing_cloud();
    let can_delete = profiles.len() > 1;

    for profile in profiles {
        let is_active = !viewing_cloud && profile.local_profile_id == active_local_id;
        render_profile_row(app, ui, &profile, is_active, can_delete, t);
        ui.add_space(5.0);
    }

    ui.add_space(3.0);
    let create_w = 88.0_f32;
    let gap = 8.0_f32;
    let input_w = (ui.available_width() - create_w - gap).max(120.0);
    let mut create_submitted = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let resp = draw_text_input(
            ui,
            InputFieldSpec {
                id_salt: "acct_new_local_profile_name",
                width: input_w,
                hint: "New local profile nickname...",
                border_color: None,
                trailing: InputTrailing::None,
            },
            &mut app.view.account.new_profile_name,
            t,
        );
        if resp.enter_pressed {
            create_submitted = true;
        }
        let has_name = !app.view.account.new_profile_name.trim().is_empty();
        if fixed_secondary_button(ui, "Add Profile", create_w, has_name, t).clicked()
            || (create_submitted && has_name)
        {
            crate::canvas::commit_pending_text(app);
            let name = app.view.account.new_profile_name.trim().to_string();
            let switch_now = !app.session.is_signed_in();
            if let Err(err) = app.session.execute(
                "profile.create",
                &json!({ "nickname": name, "switch_now": switch_now }),
            ) {
                app.view.account.error_message = Some(err.to_string());
            } else {
                app.view.account.new_profile_name.clear();
                app.view.account.error_message = None;
                crate::sync_view_to_active_notebook(app);
            }
        }
    });
}

fn render_profile_row(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    profile: &drawcraft_sync::LocalProfile,
    is_active: bool,
    can_delete: bool,
    t: &Tokens,
) {
    let is_renaming =
        app.view.account.renaming_profile_id.as_deref() == Some(&profile.local_profile_id);

    ui.horizontal(|ui| {
        if is_renaming {
            let save_w = 56.0_f32;
            let gap = 6.0_f32;
            let input_w = (ui.available_width() - save_w * 2.0 - gap * 2.0).max(100.0);
            let resp = draw_text_input(
                ui,
                InputFieldSpec {
                    id_salt: "acct_rename_profile_input",
                    width: input_w,
                    hint: "Profile nickname",
                    border_color: None,
                    trailing: InputTrailing::None,
                },
                &mut app.view.account.rename_profile_draft,
                t,
            );
            if fixed_secondary_button(ui, "Save", save_w, true, t).clicked() || resp.enter_pressed {
                let name = app.view.account.rename_profile_draft.trim().to_string();
                if let Err(err) = app.session.execute(
                    "profile.rename",
                    &json!({ "profile_id": profile.local_profile_id, "nickname": name }),
                ) {
                    app.view.account.error_message = Some(err.to_string());
                } else {
                    app.view.account.renaming_profile_id = None;
                    app.view.account.error_message = None;
                }
            }
            if fixed_secondary_button(ui, "Cancel", save_w, true, t).clicked() {
                app.view.account.renaming_profile_id = None;
            }
            return;
        }

        ui.label(
            egui::RichText::new(&profile.nickname)
                .size(12.0)
                .strong()
                .color(t.text),
        );
        ui.label(
            egui::RichText::new("Local Only")
                .size(10.5)
                .color(t.text_muted),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if can_delete && fixed_secondary_button(ui, "Delete", 58.0, true, t).clicked() {
                crate::canvas::commit_pending_text(app);
                if let Err(err) = app.session.execute(
                    "profile.delete",
                    &json!({ "profile_id": profile.local_profile_id }),
                ) {
                    app.view.account.error_message = Some(err.to_string());
                } else {
                    app.view.account.error_message = None;
                    app.view.profile_return_screen = crate::AppScreen::Home;
                    crate::sync_view_to_active_notebook(app);
                }
            }
            if fixed_secondary_button(ui, "Rename", 62.0, true, t).clicked() {
                app.view.account.renaming_profile_id = Some(profile.local_profile_id.clone());
                app.view.account.rename_profile_draft = profile.nickname.clone();
            }
            if is_active {
                draw_pill_badge(
                    ui,
                    PillStyle {
                        fill: t.accent_soft,
                        border: t.accent,
                        fg: t.accent,
                        icon: PillIcon::Dot,
                    },
                    "Active",
                );
            } else if fixed_secondary_button(ui, "Switch", 62.0, true, t).clicked() {
                crate::canvas::commit_pending_text(app);
                if let Err(err) = app.session.execute(
                    "profile.switch",
                    &json!({ "target": "local", "profile_id": profile.local_profile_id }),
                ) {
                    app.view.account.error_message = Some(err.to_string());
                } else {
                    app.view.account.error_message = None;
                    app.view.profile_return_screen = crate::AppScreen::Home;
                    crate::sync_view_to_active_notebook(app);
                }
            }
        });
    });
}

pub(crate) fn render_signed_in_transfer_section(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
) {
    let profiles = app.session.accessible_local_profiles();
    let mut entries: Vec<(String, String, drawcraft_engine::LocalNotebookEntry)> = Vec::new();
    for prof in &profiles {
        for nb in app
            .session
            .list_local_profile_notebooks(&prof.local_profile_id)
        {
            entries.push((prof.local_profile_id.clone(), prof.nickname.clone(), nb));
        }
    }

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "Move / Copy Local Notebooks to Cloud ({})",
                entries.len()
            ))
            .size(12.0)
            .strong()
            .color(t.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let lbl = if app.view.account.import_local_open {
                "Hide"
            } else {
                "Browse"
            };
            if fixed_secondary_button(ui, lbl, 68.0, !entries.is_empty(), t).clicked() {
                app.view.account.import_local_open = !app.view.account.import_local_open;
            }
        });
    });

    if !app.view.account.import_local_open || entries.is_empty() {
        return;
    }

    ui.add_space(6.0);
    for (prof_id, prof_name, nb) in entries.into_iter().take(12) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{} · {} ({}p)", nb.title, prof_name, nb.page_count))
                    .size(11.0)
                    .color(t.text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if fixed_secondary_button(ui, "Move", 56.0, true, t).clicked() {
                    run_notebook_transfer(app, ui.ctx(), &prof_id, &nb.notebook_id, true);
                }
                if fixed_secondary_button(ui, "Copy", 56.0, true, t).clicked() {
                    run_notebook_transfer(app, ui.ctx(), &prof_id, &nb.notebook_id, false);
                }
            });
        });
        ui.add_space(4.0);
    }
}

fn run_notebook_transfer(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    source_profile_id: &str,
    notebook_id: &str,
    move_notebook: bool,
) {
    match app.session.execute(
        "sync.transferNotebook",
        &json!({
            "source_profile_id": source_profile_id,
            "notebook_id": notebook_id,
            "move_notebook": move_notebook,
        }),
    ) {
        Ok(_) => {
            crate::sync_view_to_active_notebook(app);
            app.view.account.error_message = None;
            let verb = if move_notebook { "Moved" } else { "Copied" };
            app.view.account.status_message =
                Some(format!("{verb} notebook to Cloud Sync (Waiting to sync)."));
            if app.session.is_viewing_cloud() {
                trigger_manual_sync(app, ctx);
            }
        }
        Err(err) => {
            app.view.account.error_message = Some(err.to_string());
        }
    }
}

pub(crate) fn render_signed_in_server_card(
    app: &mut DrawCraftApp,
    ui: &mut egui::Ui,
    summary: &SyncStatusSummary,
    t: &Tokens,
) -> bool {
    let mut test_on_enter = false;
    let probe = app.view.account.probe.clone();
    let url_matches_saved = urls_match(&app.view.account.server_url, &summary.server_url);
    let synced_active = url_matches_saved && summary.connection == SyncConnectionState::Synced;

    ui.horizontal(|ui| {
        let icon_color = match probe {
            ServerProbeState::Connected { .. } => t.success_fg,
            _ if synced_active => t.success_fg,
            _ => t.accent,
        };
        let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_icon(ui.painter(), ir, Icon::Server, icon_color);
        ui.label(
            egui::RichText::new("Server Address")
                .size(13.0)
                .strong()
                .color(t.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if matches!(probe, ServerProbeState::Untested) && synced_active {
                draw_pill_badge(
                    ui,
                    PillStyle {
                        fill: t.success_bg,
                        border: t.success_border,
                        fg: t.success_fg,
                        icon: PillIcon::GreenCheck(t),
                    },
                    "Connected",
                );
            } else if !matches!(probe, ServerProbeState::Untested) {
                draw_probe_status_pill(ui, &probe, t);
            }
        });
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Update the host or mesh VPN IP if your server moved.")
            .size(11.0)
            .color(t.text_muted),
    );
    ui.add_space(8.0);

    let (border_override, trailing) = match &probe {
        ServerProbeState::Connected { .. } => (
            Some(t.success_border),
            InputTrailing::GreenCheck("Server connection verified"),
        ),
        ServerProbeState::Failed { .. } => (
            Some(t.danger_border),
            InputTrailing::Icon(Icon::AlertCircle, t.danger_fg, "Server connection failed"),
        ),
        ServerProbeState::Checking { .. } => (
            None,
            InputTrailing::Icon(Icon::Sync, t.accent, "Checking connection..."),
        ),
        ServerProbeState::Untested if synced_active => (
            Some(t.success_border),
            InputTrailing::GreenCheck("Active sync server"),
        ),
        ServerProbeState::Untested => (None, InputTrailing::None),
    };

    let testing = app.view.account.is_testing();
    let test_w = 72.0_f32;
    let apply_w = 76.0_f32;
    let gap = 6.0_f32;
    let input_w = (ui.available_width() - test_w - apply_w - gap * 2.0).max(120.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        let url_resp = draw_text_input(
            ui,
            InputFieldSpec {
                id_salt: "acct_server_url_signed_in",
                width: input_w,
                hint: DEFAULT_SERVER_URL,
                border_color: border_override,
                trailing,
            },
            &mut app.view.account.server_url,
            t,
        );
        if url_resp.changed {
            app.view.account.invalidate_probe_if_url_changed();
            app.view.account.error_message = None;
        }
        if url_resp.enter_pressed {
            test_on_enter = true;
        }

        let test_lbl = if testing { "..." } else { "Test" };
        if fixed_secondary_button(ui, test_lbl, test_w, !testing, t)
            .on_hover_text("Test server connection (Enter)")
            .clicked()
        {
            trigger_test_connection(app, ui.ctx());
        }

        let can_apply = !app.view.account.server_url.trim().is_empty() && !url_matches_saved;
        if fixed_secondary_button(ui, "Apply", apply_w, can_apply, t)
            .on_hover_text("Save this server address for future syncs")
            .clicked()
        {
            match app
                .session
                .set_server_url_preference(&app.view.account.server_url)
            {
                Ok(norm) => {
                    app.view.account.server_url = norm.clone();
                    app.view.account.invalidate_probe_if_url_changed();
                    app.view.account.error_message = None;
                    app.view.account.status_message =
                        Some(format!("Server URL updated to {norm}."));
                }
                Err(err) => {
                    app.view.account.status_message = None;
                    app.view.account.error_message = Some(err);
                }
            }
        }
    });

    if !matches!(probe, ServerProbeState::Untested) {
        ui.add_space(7.0);
        render_probe_feedback_row(app, ui, &probe, false, t);
    }

    test_on_enter
}

fn render_signed_in_footer(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let mut should_close = false;
    ui.horizontal(|ui| {
        if danger_action_button(ui, "Sign Out", true, t).clicked() {
            if app.session.cloud_pending_changes_count() > 0 {
                app.view.account.logout_confirm_open = true;
                app.view.account.error_message = None;
            } else {
                complete_sign_out(app, ui.ctx());
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let in_flight = app.session.is_sync_in_flight();
            let sync_lbl = if in_flight { "Syncing..." } else { "Sync Now" };
            let can_sync = !in_flight && app.session.is_viewing_cloud();
            if primary_action_button(ui, sync_lbl, can_sync, t).clicked() {
                app.view.account.error_message = None;
                app.view.account.status_message = None;
                trigger_manual_sync(app, ui.ctx());
            }
            if secondary_action_button(ui, "Close", true, t).clicked() {
                should_close = true;
            }
        });
    });
    should_close
}

fn draw_auth_mode_tabs(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let options = [
        SwitchOption::new(
            None,
            Some("Sign In"),
            "Sign in to an existing synced account",
            true,
        ),
        SwitchOption::new(
            None,
            Some("Create Account"),
            "Register a new synced account with a unique email",
            true,
        ),
    ];
    let active = match app.view.account.tab {
        AuthTab::SignIn => 0,
        AuthTab::CreateAccount => 1,
    };
    let (clicked, _) = segmented_switch(
        ui,
        "account_auth_mode_switch",
        &options,
        Some(active),
        SwitchLayout::EqualWidth,
        t,
    );
    if let Some(idx) = clicked {
        let tab = if idx == 0 {
            AuthTab::SignIn
        } else {
            AuthTab::CreateAccount
        };
        if app.view.account.tab != tab {
            app.view.account.tab = tab;
            app.view.account.error_message = None;
            app.view.account.status_message = None;
        }
    }
}

pub(crate) fn trigger_test_connection(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let raw_url = app.view.account.server_url.trim().to_string();
    match normalize_server_url(&raw_url) {
        Ok(norm) => {
            app.view.account.server_url = norm.clone();
            let _ = app.session.set_server_url_preference(&norm);
            app.view.account.error_message = None;
            app.view.account.probe = ServerProbeState::Checking {
                server_url: norm.clone(),
            };
            if !crate::sync_worker::dispatch_command(
                app,
                Some(ctx),
                SyncWorkerCommand::TestConnection {
                    server_url: norm.clone(),
                },
            ) {
                app.view.account.probe = ServerProbeState::Failed {
                    server_url: norm,
                    message: "Background network worker is unavailable.".to_string(),
                };
            }
        }
        Err(err) => {
            app.view.account.probe = ServerProbeState::Failed {
                server_url: raw_url,
                message: err.to_string(),
            };
        }
    }
}

fn submit_auth_request(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let url = match normalize_server_url(&app.view.account.server_url) {
        Ok(norm) => norm,
        Err(err) => {
            app.view.account.status_message = None;
            app.view.account.error_message = Some(err.to_string());
            return;
        }
    };
    let device_name = app.session.device_name().to_string();
    let password = app.view.account.password.clone();
    let disp_raw = app.view.account.display_name.trim().to_string();
    let display_name = (!disp_raw.is_empty()).then_some(disp_raw);

    let cmd = match app.view.account.tab {
        AuthTab::SignIn => {
            if password.is_empty() {
                app.view.account.status_message = None;
                app.view.account.error_message =
                    Some("Enter your password or recovery code.".to_string());
                return;
            }
            match build_sign_in_command(
                &url,
                &app.view.account.email_or_username,
                password,
                device_name,
            ) {
                Ok(cmd) => cmd,
                Err(msg) => {
                    app.view.account.status_message = None;
                    app.view.account.error_message = Some(msg);
                    return;
                }
            }
        }
        AuthTab::CreateAccount => {
            if let Err(err) = validate_password(&app.view.account.password) {
                app.view.account.status_message = None;
                app.view.account.error_message = Some(err.to_string());
                return;
            }
            let (email, _) = match normalize_email(&app.view.account.email_or_username) {
                Ok(pair) => pair,
                Err(err) => {
                    app.view.account.status_message = None;
                    app.view.account.error_message = Some(err.to_string());
                    return;
                }
            };
            let username = match app.view.account.signup_username.trim().is_empty() {
                true => None,
                false => match normalize_username(&app.view.account.signup_username) {
                    Ok((clean, _)) => Some(clean),
                    Err(err) => {
                        app.view.account.status_message = None;
                        app.view.account.error_message = Some(err.to_string());
                        return;
                    }
                },
            };
            SyncWorkerCommand::Register {
                server_url: url.clone(),
                email,
                username,
                password,
                display_name,
                device_name,
                mode: AuthWorkspaceMode::SwitchOnly,
            }
        }
    };

    app.view.account.server_url = url.clone();
    let _ = app.session.set_server_url_preference(&url);
    app.view.account.auth_busy = true;
    app.view.account.error_message = None;
    app.view.account.status_message = Some("Contacting DrawCraft Server...".to_string());

    if !crate::sync_worker::dispatch_command(app, Some(ctx), cmd) {
        app.view.account.auth_busy = false;
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("Background network worker is unavailable.".to_string());
    }
}

fn submit_complete_password_reset(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let url = match normalize_server_url(&app.view.account.server_url) {
        Ok(norm) => norm,
        Err(err) => {
            app.view.account.status_message = None;
            app.view.account.error_message = Some(err.to_string());
            return;
        }
    };
    let identifier = app
        .view
        .account
        .password_reset
        .identifier
        .trim()
        .to_string();
    let valid_id = if identifier.contains('@') {
        normalize_email(&identifier).map(|(e, _)| e)
    } else {
        normalize_username(&identifier).map(|(u, _)| u)
    };
    let clean_id = match valid_id {
        Ok(id) => id,
        Err(err) => {
            app.view.account.status_message = None;
            app.view.account.error_message = Some(err.to_string());
            return;
        }
    };
    let code = app
        .view
        .account
        .password_reset
        .recovery_code
        .trim()
        .to_string();
    if code.is_empty() {
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("Enter the temporary recovery code generated on the server.".to_string());
        return;
    }
    if let Err(err) = validate_password(&app.view.account.password_reset.new_password) {
        app.view.account.status_message = None;
        app.view.account.error_message = Some(err.to_string());
        return;
    }
    if app.view.account.password_reset.new_password
        != app.view.account.password_reset.confirm_password
    {
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("New password and confirmation do not match.".to_string());
        return;
    }
    if app.view.account.password_reset.new_password == code {
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("New password must be different from the temporary recovery code.".to_string());
        return;
    }

    let device_name = app.session.device_name().to_string();
    let new_password = app.view.account.password_reset.new_password.clone();
    app.view.account.server_url = url.clone();
    let _ = app.session.set_server_url_preference(&url);
    app.view.account.password_reset.busy = true;
    app.view.account.error_message = None;
    app.view.account.status_message =
        Some("Verifying recovery code and saving your new password...".to_string());

    let cmd = SyncWorkerCommand::CompletePasswordReset {
        server_url: url,
        identifier: clean_id,
        recovery_code: code,
        new_password,
        device_name,
        mode: AuthWorkspaceMode::SwitchOnly,
    };
    if !crate::sync_worker::dispatch_command(app, Some(ctx), cmd) {
        app.view.account.password_reset.busy = false;
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("Background network worker is unavailable.".to_string());
    }
}

fn submit_update_profile(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let Some(session) = app.session.active_account_session().cloned() else {
        app.view.account.error_message = Some("Sign in to a cloud account first.".to_string());
        return;
    };
    let (clean_username, _) = match normalize_username(&app.view.account.edit_username) {
        Ok(pair) => pair,
        Err(err) => {
            app.view.account.status_message = None;
            app.view.account.error_message = Some(err.to_string());
            return;
        }
    };
    let clean_display = sanitize_display_name(&app.view.account.edit_display_name, &clean_username);
    let username = (clean_username != session.user.username).then_some(clean_username);
    let display_name = (clean_display != session.user.display_name).then_some(clean_display);
    if username.is_none() && display_name.is_none() {
        app.view.account.status_message = Some("No profile changes to save.".to_string());
        return;
    }

    app.view.account.profile_save_busy = true;
    app.view.account.error_message = None;
    let cmd = SyncWorkerCommand::UpdateProfile {
        session,
        username,
        display_name,
    };
    if !crate::sync_worker::dispatch_command(app, Some(ctx), cmd) {
        app.view.account.profile_save_busy = false;
        app.view.account.error_message =
            Some("Background network worker is unavailable.".to_string());
    }
}

fn submit_change_password(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let Some(session) = app.session.active_account_session().cloned() else {
        app.view.account.error_message = Some("Sign in to a cloud account first.".to_string());
        return;
    };
    if app.view.account.current_password_draft.is_empty() {
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("Enter your current password to confirm.".to_string());
        return;
    }
    if let Err(err) = validate_password(&app.view.account.new_password_draft) {
        app.view.account.status_message = None;
        app.view.account.error_message = Some(err.to_string());
        return;
    }
    if app.view.account.new_password_draft != app.view.account.confirm_new_password_draft {
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("New password and confirmation do not match.".to_string());
        return;
    }
    if app.view.account.new_password_draft == app.view.account.current_password_draft {
        app.view.account.status_message = None;
        app.view.account.error_message =
            Some("New password must be different from your current password.".to_string());
        return;
    }

    app.view.account.change_password_busy = true;
    app.view.account.error_message = None;
    let cmd = SyncWorkerCommand::ChangePassword {
        session,
        current_password: app.view.account.current_password_draft.clone(),
        new_password: app.view.account.new_password_draft.clone(),
    };
    if !crate::sync_worker::dispatch_command(app, Some(ctx), cmd) {
        app.view.account.change_password_busy = false;
        app.view.account.error_message =
            Some("Background network worker is unavailable.".to_string());
    }
}

fn build_sign_in_command(
    server_url: &str,
    identifier_raw: &str,
    password: String,
    device_name: String,
) -> Result<SyncWorkerCommand, String> {
    let trimmed = identifier_raw.trim();
    if trimmed.contains('@') {
        let (email, _) = normalize_email(trimmed).map_err(|e| e.to_string())?;
        Ok(SyncWorkerCommand::Login {
            server_url: server_url.to_string(),
            identifier: email,
            password,
            device_name,
            mode: AuthWorkspaceMode::SwitchOnly,
        })
    } else {
        let (username, _) = normalize_username(trimmed).map_err(|e| e.to_string())?;
        Ok(SyncWorkerCommand::Login {
            server_url: server_url.to_string(),
            identifier: username,
            password,
            device_name,
            mode: AuthWorkspaceMode::SwitchOnly,
        })
    }
}

fn submit_change_email(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let Some(session) = app.session.active_account_session().cloned() else {
        app.view.account.error_message = Some("Sign in to a cloud account first.".to_string());
        return;
    };
    let (new_email, _) = match normalize_email(&app.view.account.new_email_draft) {
        Ok(pair) => pair,
        Err(err) => {
            app.view.account.error_message = Some(err.to_string());
            return;
        }
    };
    if app.view.account.change_email_password.is_empty() {
        app.view.account.error_message =
            Some("Enter your current password to confirm.".to_string());
        return;
    }

    app.view.account.change_email_busy = true;
    app.view.account.error_message = None;
    let cmd = SyncWorkerCommand::ChangeEmail {
        session,
        new_email,
        current_password: app.view.account.change_email_password.clone(),
    };
    if !crate::sync_worker::dispatch_command(app, Some(ctx), cmd) {
        app.view.account.change_email_busy = false;
        app.view.account.error_message =
            Some("Background network worker is unavailable.".to_string());
    }
}

enum FieldHint {
    Muted(&'static str),
    Success(&'static str),
    Warning(String),
}

fn draw_field_label_row(ui: &mut egui::Ui, label: &str, hint: FieldHint, t: &Tokens) {
    ui.horizontal_top(|ui| {
        ui.label(egui::RichText::new(label).size(11.5).strong().color(t.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let (text, color) = match hint {
                FieldHint::Muted(s) => (s.to_string(), t.text_muted),
                FieldHint::Success(s) => (s.to_string(), t.success_fg),
                FieldHint::Warning(s) => (s, t.warning_fg),
            };
            ui.label(egui::RichText::new(text).size(10.5).color(color));
        });
    });
}

enum InputTrailing {
    None,
    GreenCheck(&'static str),
    Icon(Icon, Color32, &'static str),
}

struct InputFieldSpec<'a> {
    id_salt: &'a str,
    width: f32,
    hint: &'a str,
    border_color: Option<Color32>,
    trailing: InputTrailing,
}

struct InputBoxOutcome {
    changed: bool,
    enter_pressed: bool,
}

fn draw_input_shell(
    ui: &mut egui::Ui,
    id_salt: &str,
    width: f32,
    border_override: Option<Color32>,
    t: &Tokens,
) -> (Rect, egui::Id) {
    let input_width = width.max(0.0).min(ui.available_width().max(0.0));
    let (box_rect, box_resp) =
        ui.allocate_exact_size(Vec2::new(input_width, INPUT_HEIGHT), Sense::hover());
    let edit_id = ui.make_persistent_id(id_salt);
    let has_focus = ui.ctx().memory(|m| m.has_focus(edit_id));
    let stroke = if has_focus {
        Stroke::new(1.5, t.accent)
    } else if box_resp.hovered() {
        Stroke::new(1.0, t.text_muted.gamma_multiply(0.55))
    } else if let Some(custom) = border_override {
        Stroke::new(1.2, custom)
    } else {
        Stroke::new(1.0, t.border)
    };
    let p = ui.painter();
    p.rect_filled(
        box_rect,
        CornerRadius::same(CONTROL_RADIUS as u8),
        t.field_bg,
    );
    p.rect_stroke(
        box_rect,
        CornerRadius::same(CONTROL_RADIUS as u8),
        stroke,
        StrokeKind::Inside,
    );
    (box_rect, edit_id)
}

fn draw_text_input(
    ui: &mut egui::Ui,
    spec: InputFieldSpec<'_>,
    value: &mut String,
    t: &Tokens,
) -> InputBoxOutcome {
    let (box_rect, edit_id) = draw_input_shell(ui, spec.id_salt, spec.width, spec.border_color, t);
    let inner_rect = box_rect.shrink2(Vec2::new(10.0, 2.0));
    let has_trailing = !matches!(spec.trailing, InputTrailing::None);
    let trailing_w = if has_trailing { 22.0 } else { 0.0 };
    let text_w = (inner_rect.width() - trailing_w).max(40.0);

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 6.0;
    child.set_width(inner_rect.width());

    let resp = child.add_sized(
        Vec2::new(text_w, inner_rect.height()),
        egui::TextEdit::singleline(value)
            .id(edit_id)
            .desired_width(text_w)
            .font(egui::FontId::proportional(12.5))
            .margin(egui::Margin::ZERO)
            .align(egui::Align2::LEFT_CENTER)
            .frame(egui::Frame::NONE)
            .hint_text(spec.hint),
    );
    let enter_pressed = resp.lost_focus() && child.input(|i| i.key_pressed(egui::Key::Enter));

    match spec.trailing {
        InputTrailing::None => {}
        InputTrailing::GreenCheck(tip) => {
            let (ir, icon_resp) = child.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            paint_green_check_badge(child.painter(), ir, t);
            let _ = icon_resp.on_hover_text(tip);
        }
        InputTrailing::Icon(icon, color, tip) => {
            let (ir, icon_resp) = child.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
            paint_icon(child.painter(), ir, icon, color);
            let _ = icon_resp.on_hover_text(tip);
        }
    }

    InputBoxOutcome {
        changed: resp.changed(),
        enter_pressed,
    }
}

fn draw_password_input(
    ui: &mut egui::Ui,
    id_salt: &str,
    width: f32,
    password: &mut String,
    show_password: &mut bool,
    t: &Tokens,
) -> InputBoxOutcome {
    let (box_rect, edit_id) = draw_input_shell(ui, id_salt, width, None, t);
    let inner_rect = box_rect.shrink2(Vec2::new(10.0, 2.0));
    let toggle_w = 24.0_f32;
    let text_w = (inner_rect.width() - toggle_w - 4.0).max(40.0);

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    child.set_width(inner_rect.width());

    let resp = child.add_sized(
        Vec2::new(text_w, inner_rect.height()),
        egui::TextEdit::singleline(password)
            .id(edit_id)
            .password(!*show_password)
            .desired_width(text_w)
            .font(egui::FontId::proportional(12.5))
            .margin(egui::Margin::ZERO)
            .align(egui::Align2::LEFT_CENTER)
            .frame(egui::Frame::NONE)
            .hint_text("Minimum 8 characters"),
    );
    let enter_pressed = resp.lost_focus() && child.input(|i| i.key_pressed(egui::Key::Enter));

    let (btn_rect, btn_resp) = child.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    if child.is_rect_visible(btn_rect) {
        let hover_t =
            crate::motion::hover_factor(child.ctx(), btn_resp.id.with("hov"), btn_resp.hovered());
        let bg = crate::motion::lerp_color(Color32::TRANSPARENT, t.toolbar_bg, hover_t);
        let fg = crate::motion::lerp_color(t.text_muted, t.text, hover_t);
        child
            .painter()
            .rect_filled(btn_rect, CornerRadius::same(5), bg);
        let icon = if *show_password {
            Icon::EyeOff
        } else {
            Icon::Eye
        };
        paint_icon(child.painter(), btn_rect.shrink(3.5), icon, fg);
    }
    let tip = if *show_password {
        "Hide password"
    } else {
        "Show password"
    };
    if btn_resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
        .clicked()
    {
        *show_password = !*show_password;
    }

    InputBoxOutcome {
        changed: resp.changed(),
        enter_pressed,
    }
}

pub(crate) fn fixed_secondary_button(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    enabled: bool,
    t: &Tokens,
) -> egui::Response {
    let text_color = if enabled { t.text } else { t.text_muted };
    let btn = egui::Button::new(egui::RichText::new(label).size(12.0).color(text_color))
        .fill(t.toolbar_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CONTROL_RADIUS)
        .min_size(Vec2::new(width, INPUT_HEIGHT));
    let resp = ui.add_enabled(enabled, btn);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

pub(crate) fn draw_section_card(ui: &mut egui::Ui, t: &Tokens, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CARD_RADIUS)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            body(ui);
        });
}

pub(crate) fn draw_hairline(ui: &mut egui::Ui, t: &Tokens) {
    let w = ui.available_width();
    let (sep_rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(
        sep_rect.x_range(),
        sep_rect.center().y,
        Stroke::new(1.0, t.border),
    );
}

fn draw_metric_cell(ui: &mut egui::Ui, label: &str, value: &str, t: &Tokens) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(egui::RichText::new(label).size(10.0).color(t.text_muted));
        ui.label(egui::RichText::new(value).size(12.0).strong().color(t.text));
    });
}

pub(crate) fn render_auth_banners(app: &DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    if let Some(ref err) = app.view.account.error_message {
        ui.add_space(8.0);
        draw_status_banner(ui, BannerTone::Error, err, t);
    } else if let Some(ref msg) = app.view.account.status_message {
        ui.add_space(8.0);
        let tone = if msg.starts_with("Server URL updated")
            || msg.starts_with("Account email updated")
            || msg.starts_with("Profile updated")
            || msg.starts_with("Account password updated")
            || msg.starts_with("Moved ")
            || msg.starts_with("Copied ")
        {
            BannerTone::Success
        } else {
            BannerTone::Info
        };
        draw_status_banner(ui, tone, msg, t);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BannerTone {
    Info,
    Success,
    Warning,
    Error,
}

pub(crate) fn draw_status_banner(ui: &mut egui::Ui, tone: BannerTone, text: &str, t: &Tokens) {
    let (bg, border, fg) = match tone {
        BannerTone::Info => (t.accent_soft, t.accent, t.text),
        BannerTone::Success => (t.success_bg, t.success_border, t.success_fg),
        BannerTone::Warning => (t.toolbar_bg, t.border, t.warning_fg),
        BannerTone::Error => (t.danger_bg, t.danger_border, t.danger_fg),
    };
    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                match tone {
                    BannerTone::Success => paint_green_check_badge(ui.painter(), icon_rect, t),
                    BannerTone::Info => paint_icon(ui.painter(), icon_rect, Icon::Info, t.accent),
                    BannerTone::Warning => {
                        paint_icon(ui.painter(), icon_rect, Icon::AlertCircle, t.warning_fg);
                    }
                    BannerTone::Error => {
                        paint_icon(ui.painter(), icon_rect, Icon::AlertCircle, t.danger_fg);
                    }
                }
                ui.add(egui::Label::new(egui::RichText::new(text).size(11.5).color(fg)).wrap());
            });
        });
}

pub(crate) enum PillIcon<'a> {
    None,
    Vector(Icon),
    GreenCheck(&'a Tokens),
    Dot,
}

pub(crate) struct PillStyle<'a> {
    pub(crate) fill: Color32,
    pub(crate) border: Color32,
    pub(crate) fg: Color32,
    pub(crate) icon: PillIcon<'a>,
}

pub(crate) fn draw_pill_badge(ui: &mut egui::Ui, style: PillStyle<'_>, label: &str) {
    egui::Frame::new()
        .fill(style.fill)
        .stroke(Stroke::new(1.0, style.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                match style.icon {
                    PillIcon::None => {}
                    PillIcon::Vector(icon) => {
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                        paint_icon(ui.painter(), ir, icon, style.fg);
                    }
                    PillIcon::GreenCheck(t) => {
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        paint_green_check_badge(ui.painter(), ir, t);
                    }
                    PillIcon::Dot => {
                        let (dr, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                        ui.painter().circle_filled(dr.center(), 3.5, style.fg);
                    }
                }
                ui.label(
                    egui::RichText::new(label)
                        .size(11.0)
                        .strong()
                        .color(style.fg),
                );
            });
        });
}

pub(crate) fn draw_sync_connection_pill(ui: &mut egui::Ui, state: SyncConnectionState, t: &Tokens) {
    let (label, fg) = status_badge_style(state, t);
    let style = match state {
        SyncConnectionState::Synced => PillStyle {
            fill: t.success_bg,
            border: t.success_border,
            fg,
            icon: PillIcon::GreenCheck(t),
        },
        SyncConnectionState::Syncing => PillStyle {
            fill: t.accent_soft,
            border: t.accent,
            fg,
            icon: PillIcon::Vector(Icon::Sync),
        },
        SyncConnectionState::Error => PillStyle {
            fill: t.danger_bg,
            border: t.danger_border,
            fg,
            icon: PillIcon::Vector(Icon::AlertCircle),
        },
        SyncConnectionState::Offline | SyncConnectionState::SignedOut => PillStyle {
            fill: t.toolbar_bg,
            border: t.border,
            fg,
            icon: PillIcon::Dot,
        },
    };
    draw_pill_badge(ui, style, label);
}

/// Paints a solid green circular status badge containing a crisp white checkmark.
pub(crate) fn paint_green_check_badge(painter: &egui::Painter, rect: Rect, t: &Tokens) {
    let radius = rect.width().min(rect.height()) * 0.5;
    painter.circle_filled(rect.center(), radius, t.success_fg);
    paint_icon(
        painter,
        rect.shrink(radius * 0.28),
        Icon::Check,
        Color32::WHITE,
    );
}

pub(crate) fn status_badge_style(
    state: SyncConnectionState,
    t: &Tokens,
) -> (&'static str, Color32) {
    match state {
        SyncConnectionState::SignedOut => ("Local Only", t.text_muted),
        SyncConnectionState::Offline => ("Offline", t.warning_fg),
        SyncConnectionState::Syncing => ("Syncing", t.accent),
        SyncConnectionState::Synced => ("Synced", t.success_fg),
        SyncConnectionState::Error => ("Sync Issue", t.danger_fg),
    }
}

/// Returns badge text and semantic color tokens for per-notebook sync indicators.
pub(crate) fn notebook_status_badge_style(
    status: EntitySyncStatus,
    t: &Tokens,
) -> (&'static str, Color32, Color32, Color32) {
    match status {
        EntitySyncStatus::LocalOnly => ("Local only", t.text_muted, t.toolbar_bg, t.border),
        EntitySyncStatus::PendingPush => ("Waiting to sync", t.warning_fg, t.toolbar_bg, t.border),
        EntitySyncStatus::Synced => ("Synced", t.success_fg, t.success_bg, t.success_border),
        EntitySyncStatus::Conflict => ("Conflict", t.danger_fg, t.danger_bg, t.danger_border),
        EntitySyncStatus::SyncError => ("Sync error", t.danger_fg, t.danger_bg, t.danger_border),
    }
}

fn urls_match(a: &str, b: &str) -> bool {
    match (normalize_server_url(a), normalize_server_url(b)) {
        (Ok(na), Ok(nb)) => na == nb,
        _ => a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/'),
    }
}

fn format_last_sync(last_ms: Option<i64>) -> String {
    let Some(ts) = last_ms else {
        return "Not synced yet".to_string();
    };
    let elapsed_s = (now_unix_ms() - ts).max(0) / 1000;
    if elapsed_s < 10 {
        "Just now".to_string()
    } else if elapsed_s < 60 {
        format!("{elapsed_s}s ago")
    } else if elapsed_s < 3600 {
        format!("{}m ago", elapsed_s / 60)
    } else {
        format!("{}h ago", elapsed_s / 3600)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{AuthTab, ServerProbeState};
    use crate::DrawCraftApp;
    use crate::modal;
    use drawcraft_engine::AuthWorkspaceMode;
    use drawcraft_sync::{ActiveAccountSession, SyncConflictPreview};
    use drawcraft_sync_protocol::{
        DEFAULT_SERVER_URL, LocalDiscoverySummary, ServerInfoResponse, UserProfile,
    };
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

    fn count_filled_circles_with_color(shapes: &[ClippedShape], target: egui::Color32) -> usize {
        let mut count = 0usize;
        fn visit(shape: &Shape, target: egui::Color32, count: &mut usize) {
            match shape {
                Shape::Circle(c) if c.fill == target => {
                    *count += 1;
                }
                Shape::Vec(items) => {
                    for item in items {
                        visit(item, target, count);
                    }
                }
                _ => {}
            }
        }
        for clipped in shapes {
            visit(&clipped.shape, target, &mut count);
        }
        count
    }

    fn test_account(user_id: &str, email: &str) -> ActiveAccountSession {
        ActiveAccountSession {
            server_url: DEFAULT_SERVER_URL.to_string(),
            user: UserProfile {
                user_id: user_id.to_string(),
                email: Some(email.to_string()),
                username: "alex".to_string(),
                display_name: "Alex".to_string(),
                created_at_ms: 1000,
                must_change_password: false,
            },
            access_token: "a".to_string(),
            refresh_token: "r".to_string(),
            access_expires_at_ms: 99_999,
            refresh_expires_at_ms: 99_999,
            device_name: "WinPC".to_string(),
            last_auth_ms: 1000,
        }
    }

    fn render_modal_settled(ctx: &egui::Context, app: &mut DrawCraftApp) -> String {
        let mut text = String::new();
        for step in 0..6 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(f64::from(step) * 0.08),
                predicted_dt: 0.08,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                modal::show(app, ui);
            });
            text = collect_text(&out.shapes);
            out.drop_without_applying_deltas();
        }
        text
    }

    #[test]
    fn account_sync_modal_renders_both_signed_out_and_signed_in_states() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.session.prefs.theme = drawcraft_doc::UiTheme::Light;
        modal::open(&mut app, modal::ActiveModal::AccountSync);

        let signed_out_text = render_modal_settled(&ctx, &mut app);
        assert_eq!(app.view.active_modal, Some(modal::ActiveModal::AccountSync));
        assert!(
            signed_out_text.contains("On This Device:"),
            "signed-out text:\n{signed_out_text}"
        );
        assert!(!signed_out_text.contains("Link Profile"));
        assert!(!signed_out_text.contains("Do you want to sync it"));

        let _ = app.session.execute(
            "profile.create",
            &serde_json::json!({ "nickname": "Bruno" }),
        );
        app.session.switch_to_authenticated_library(
            test_account("u-1", "alex@example.com"),
            AuthWorkspaceMode::SwitchOnly,
        );
        let signed_in_text = render_modal_settled(&ctx, &mut app);
        assert!(app.session.is_signed_in());
        assert!(
            signed_in_text.contains("alex@example.com"),
            "signed-in text:\n{signed_in_text}"
        );
        assert!(signed_in_text.contains("PENDING UPLOADS"));
        assert!(
            signed_in_text.contains("A local account 'Bruno' was found"),
            "missing local account migration prompt in signed-in modal:\n{signed_in_text}"
        );
    }

    #[test]
    fn first_access_and_conflict_modals_render_cleanly() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();

        modal::open(&mut app, modal::ActiveModal::FirstAccessNickname);
        let first_text = render_modal_settled(&ctx, &mut app);
        assert!(first_text.contains("Nickname"));
        assert!(first_text.contains("Start Session"));

        app.view.account.pending_conflict = Some(SyncConflictPreview {
            account_name: "Ada".to_string(),
            account_email: "ada@example.com".to_string(),
            server_notebook_count: 3,
            local_notebook_count: 2,
            server_cursor: 7,
            local_pending_changes: 1,
            local_dirty_titles: vec!["Offline Draft".to_string()],
            server_notebook_titles: vec!["Server Copy".to_string()],
            offline_edits_detected: true,
        });
        modal::open(&mut app, modal::ActiveModal::SyncConflict);
        let conflict_text = render_modal_settled(&ctx, &mut app);
        assert!(conflict_text.contains("Server Version"));
        assert!(conflict_text.contains("Local Version (from this computer)"));
    }

    #[test]
    fn logout_safeguard_warns_when_cloud_has_pending_push_changes() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.session.prefs.theme = drawcraft_doc::UiTheme::Light;
        modal::open(&mut app, modal::ActiveModal::AccountSync);
        app.session.switch_to_authenticated_library(
            test_account("u-3", "sam@example.com"),
            AuthWorkspaceMode::SwitchOnly,
        );
        app.view.account.logout_confirm_open = true;

        let text = render_modal_settled(&ctx, &mut app);
        assert!(
            text.contains("Unsynced Cloud Changes"),
            "logout safeguard text:\n{text}"
        );
        assert!(text.contains("Sync & Sign Out"));
        assert!(text.contains("Sign Out Anyway"));
    }

    #[test]
    fn successful_server_probe_shows_green_icon_and_resets_when_url_changes() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.session.prefs.theme = drawcraft_doc::UiTheme::Light;
        modal::open(&mut app, modal::ActiveModal::AccountSync);

        app.view.account.apply_health_ok(
            DEFAULT_SERVER_URL.to_string(),
            ServerInfoResponse {
                status: "ok".to_string(),
                service: "drawcraft-server".to_string(),
                version: "0.1.0".to_string(),
                protocol_version: 1,
                registration_open: true,
                storage_engine: "redb".to_string(),
                max_notebook_bytes: 1_048_576,
                local_discovery: LocalDiscoverySummary::default(),
            },
        );
        assert!(matches!(
            app.view.account.probe,
            ServerProbeState::Connected { .. }
        ));

        let mut text_out = String::new();
        let mut green_badges = 0usize;
        for step in 0..6 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(f64::from(step) * 0.08),
                predicted_dt: 0.08,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_light_visuals(ui.ctx());
                modal::show(&mut app, ui);
            });
            text_out = collect_text(&out.shapes);
            let light = crate::theme::Tokens::light();
            green_badges = count_filled_circles_with_color(&out.shapes, light.success_fg);
            out.drop_without_applying_deltas();
        }
        assert!(text_out.contains("Connected to DrawCraft Server v0.1.0"));
        assert!(text_out.contains("Registration open"));
        assert!(
            green_badges >= 2,
            "expected green connection icons in header/input/banner, got {green_badges}"
        );

        app.view.account.server_url = "http://100.64.0.10:8787".to_string();
        app.view.account.invalidate_probe_if_url_changed();
        assert_eq!(app.view.account.probe, ServerProbeState::Untested);
    }

    #[test]
    fn closed_registration_probe_flags_create_account_tab() {
        let mut app = DrawCraftApp::default();
        app.view.account.tab = AuthTab::CreateAccount;
        app.view.account.apply_health_ok(
            DEFAULT_SERVER_URL.to_string(),
            ServerInfoResponse {
                status: "ok".to_string(),
                service: "drawcraft-server".to_string(),
                version: "0.1.0".to_string(),
                protocol_version: 1,
                registration_open: false,
                storage_engine: "redb".to_string(),
                max_notebook_bytes: 1_048_576,
                local_discovery: LocalDiscoverySummary::default(),
            },
        );
        assert!(app.view.account.is_registration_closed());

        app.view.account.server_url = "http://100.64.0.22:8787".to_string();
        app.view.account.invalidate_probe_if_url_changed();
        app.view
            .account
            .apply_health_err(DEFAULT_SERVER_URL.to_string(), "timeout".to_string());
        assert_eq!(app.view.account.probe, ServerProbeState::Untested);
    }

    #[test]
    fn dark_theme_and_create_account_with_banners_fit_without_clipping() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        let _ = app
            .session
            .execute("theme.set", &serde_json::json!({ "theme": "Dark" }));
        modal::open(&mut app, modal::ActiveModal::AccountSync);
        app.view.account.tab = AuthTab::CreateAccount;
        app.view.account.apply_health_ok(
            DEFAULT_SERVER_URL.to_string(),
            ServerInfoResponse {
                status: "ok".to_string(),
                service: "drawcraft-server".to_string(),
                version: "0.1.0".to_string(),
                protocol_version: 1,
                registration_open: true,
                storage_engine: "redb".to_string(),
                max_notebook_bytes: 1_048_576,
                local_discovery: LocalDiscoverySummary::default(),
            },
        );
        app.view.account.error_message = Some("Email already taken".to_string());

        let mut text_out = String::new();
        let mut green_badges = 0usize;
        for step in 0..6 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                time: Some(f64::from(step) * 0.08),
                predicted_dt: 0.08,
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                crate::theme::apply_visuals(ui.ctx(), drawcraft_doc::UiTheme::Dark);
                modal::show(&mut app, ui);
            });
            text_out = collect_text(&out.shapes);
            let dark = crate::theme::Tokens::dark();
            green_badges = count_filled_circles_with_color(&out.shapes, dark.success_fg);
            out.drop_without_applying_deltas();
        }
        assert!(
            text_out.contains("Display Name"),
            "missing Display Name in:\n{text_out}"
        );
        assert!(
            text_out.contains("Connected to DrawCraft Server v0.1.0"),
            "missing Connected banner in:\n{text_out}"
        );
        assert!(
            text_out.contains("Email already taken"),
            "missing error banner in:\n{text_out}"
        );
        assert!(
            green_badges >= 2,
            "expected dark-theme green connection badges, got {green_badges}"
        );
    }

    #[test]
    fn account_modal_local_profile_create_switch_rename_and_delete_succeed() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.session
            .execute(
                "profile.completeFirstAccess",
                &serde_json::json!({ "nickname": "Primary" }),
            )
            .unwrap();
        let primary_id = app
            .session
            .active_local_profile()
            .unwrap()
            .local_profile_id
            .clone();

        let created = app
            .session
            .execute(
                "profile.create",
                &serde_json::json!({ "nickname": "test", "switch_now": true }),
            )
            .unwrap();
        let test_id = created["localProfileId"].as_str().unwrap().to_string();
        assert_eq!(app.session.active_local_profile().unwrap().nickname, "test");

        app.session
            .execute(
                "profile.switch",
                &serde_json::json!({ "target": "local", "profile_id": primary_id }),
            )
            .unwrap();
        assert_eq!(
            app.session.active_local_profile().unwrap().nickname,
            "Primary"
        );

        app.session
            .execute(
                "profile.rename",
                &serde_json::json!({ "profile_id": test_id, "nickname": "test2" }),
            )
            .unwrap();

        modal::open(&mut app, modal::ActiveModal::AccountSync);
        app.view.account.profiles_panel_open = true;
        let text = render_modal_settled(&ctx, &mut app);
        assert!(text.contains("On This Device: Primary"), "text:\n{text}");
        assert!(text.contains("test2"), "text:\n{text}");

        app.session
            .execute(
                "profile.delete",
                &serde_json::json!({ "profile_id": test_id }),
            )
            .unwrap();
        assert_eq!(app.session.accessible_local_profiles().len(), 1);
    }
}
