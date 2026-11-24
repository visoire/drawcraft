use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use drawcraft_engine::AuthWorkspaceMode;
use drawcraft_sync::{ActiveAccountSession, SyncBatch, SyncConflictPreview, SyncCycleOutcome};
#[cfg(not(target_arch = "wasm32"))]
use drawcraft_sync::{
    SyncError, SyncHttpClient, build_conflict_preview_if_needed, outcome_from_full_pull,
    run_sync_cycle, run_sync_cycle_keep_local,
};
use drawcraft_sync_protocol::{
    ChangeEmailRequest, ChangePasswordRequest, CompletePasswordResetRequest, LocalDiscoveryReport,
    LoginRequest, RegisterRequest, ServerInfoResponse, UpdateProfileRequest, normalize_email,
    normalize_username, now_unix_ms, sanitize_display_name, validate_password,
};
#[cfg(not(target_arch = "wasm32"))]
use drawcraft_sync_protocol::{
    ERR_CODE_PASSWORD_CHANGE_REQUIRED, ERR_CODE_PASSWORD_RESET_REQUIRED,
};

use crate::DrawCraftApp;

const AUTO_SYNC_DEBOUNCE_MS: f64 = 8_000.0;
const AUTO_SYNC_IDLE_PULL_MS: f64 = 60_000.0;
const DISCOVERY_REPORT_INTERVAL_MS: f64 = 45_000.0;

pub enum SyncWorkerCommand {
    TestConnection {
        server_url: String,
    },
    ReportLocalDiscovery {
        server_url: String,
        report: LocalDiscoveryReport,
    },
    Register {
        server_url: String,
        email: String,
        username: Option<String>,
        password: String,
        display_name: Option<String>,
        device_name: String,
        mode: AuthWorkspaceMode,
    },
    Login {
        server_url: String,
        identifier: String,
        password: String,
        device_name: String,
        mode: AuthWorkspaceMode,
    },
    CompletePasswordReset {
        server_url: String,
        identifier: String,
        recovery_code: String,
        new_password: String,
        device_name: String,
        mode: AuthWorkspaceMode,
    },
    UpdateProfile {
        session: ActiveAccountSession,
        username: Option<String>,
        display_name: Option<String>,
    },
    ChangePassword {
        session: ActiveAccountSession,
        current_password: String,
        new_password: String,
    },
    ChangeEmail {
        session: ActiveAccountSession,
        new_email: String,
        current_password: String,
    },
    SyncNow {
        session: ActiveAccountSession,
        batch: Box<SyncBatch>,
        offline_edits_pending: bool,
    },
    ResolveConflictKeepServer {
        session: ActiveAccountSession,
    },
    ResolveConflictKeepLocal {
        session: ActiveAccountSession,
        batch: Box<SyncBatch>,
    },
    Logout {
        session: ActiveAccountSession,
    },
}

pub enum SyncWorkerEvent {
    HealthOk {
        server_url: String,
        info: ServerInfoResponse,
    },
    HealthErr {
        server_url: String,
        message: String,
    },
    AuthOk {
        auth: ActiveAccountSession,
        mode: AuthWorkspaceMode,
    },
    AuthErr {
        message: String,
    },
    PasswordResetRequired {
        identifier: String,
        prefill_code: Option<String>,
        message: String,
    },
    ProfileUpdated {
        updated_session: ActiveAccountSession,
    },
    ProfileUpdateErr {
        message: String,
    },
    PasswordChanged {
        updated_session: ActiveAccountSession,
    },
    PasswordChangeErr {
        message: String,
    },
    EmailChanged {
        updated_session: ActiveAccountSession,
    },
    EmailChangeErr {
        message: String,
    },
    SyncOk {
        outcome: Box<SyncCycleOutcome>,
    },
    ConflictDecisionRequired {
        preview: SyncConflictPreview,
    },
    ConflictResolvedKeepServer {
        outcome: Box<SyncCycleOutcome>,
    },
    SyncErr {
        offline: bool,
        unauthorized: bool,
        message: String,
    },
}

/// Builds a normalized registration payload from an email address, optional username, and password.
pub fn build_register_request(
    email_raw: &str,
    username_raw: Option<&str>,
    password: String,
    display_name: Option<String>,
    device_name: &str,
) -> Result<RegisterRequest, String> {
    let (email, _) = normalize_email(email_raw).map_err(|e| e.to_string())?;
    let username = match username_raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => normalize_username(raw).map_err(|e| e.to_string())?.0,
        None => String::new(),
    };
    Ok(RegisterRequest {
        email: Some(email),
        username,
        password,
        display_name,
        device_name: Some(device_name.to_string()),
    })
}

/// Builds a login payload accepting either an email address or a username.
pub fn build_login_request(
    identifier: &str,
    password: String,
    device_name: &str,
) -> Result<LoginRequest, String> {
    let trimmed = identifier.trim();
    if trimmed.contains('@') {
        let (email, _) = normalize_email(trimmed).map_err(|e| e.to_string())?;
        Ok(LoginRequest {
            email: Some(email),
            username: String::new(),
            password,
            device_name: Some(device_name.to_string()),
        })
    } else {
        let (username, _) = normalize_username(trimmed).map_err(|e| e.to_string())?;
        Ok(LoginRequest {
            email: None,
            username,
            password,
            device_name: Some(device_name.to_string()),
        })
    }
}

/// Builds a validated `CompletePasswordResetRequest` payload.
pub fn build_complete_password_reset_request(
    identifier: &str,
    recovery_code: &str,
    new_password: String,
    device_name: &str,
) -> Result<CompletePasswordResetRequest, String> {
    let trimmed = identifier.trim();
    let code = recovery_code.trim();
    if code.is_empty() {
        return Err("Enter the temporary recovery code generated on the server.".to_string());
    }
    validate_password(&new_password).map_err(|e| e.to_string())?;
    if new_password == code {
        return Err("New password must be different from the temporary recovery code.".to_string());
    }
    let (email, username) = if trimmed.contains('@') {
        let (email, _) = normalize_email(trimmed).map_err(|e| e.to_string())?;
        (Some(email), String::new())
    } else {
        let (username, _) = normalize_username(trimmed).map_err(|e| e.to_string())?;
        (None, username)
    };
    Ok(CompletePasswordResetRequest {
        email,
        username,
        recovery_code: code.to_string(),
        new_password,
        device_name: Some(device_name.to_string()),
    })
}

/// Builds a validated `UpdateProfileRequest` payload.
pub fn build_update_profile_request(
    username_raw: Option<&str>,
    display_name_raw: Option<&str>,
) -> Result<UpdateProfileRequest, String> {
    let username = match username_raw {
        Some(raw) => Some(normalize_username(raw).map_err(|e| e.to_string())?.0),
        None => None,
    };
    let display_name = display_name_raw.map(|raw| sanitize_display_name(raw, ""));
    if username.is_none() && display_name.is_none() {
        return Err("No profile changes to save.".to_string());
    }
    Ok(UpdateProfileRequest {
        username,
        display_name,
    })
}

/// Builds a validated `ChangePasswordRequest` payload.
pub fn build_change_password_request(
    current_password: String,
    new_password: String,
) -> Result<ChangePasswordRequest, String> {
    if current_password.is_empty() {
        return Err("Enter your current password to confirm.".to_string());
    }
    validate_password(&new_password).map_err(|e| e.to_string())?;
    if new_password == current_password {
        return Err("New password must be different from your current password.".to_string());
    }
    Ok(ChangePasswordRequest {
        current_password,
        new_password,
    })
}

/// Builds a validated `ChangeEmailRequest` payload.
pub fn build_change_email_request(
    new_email_raw: &str,
    current_password: String,
) -> Result<ChangeEmailRequest, String> {
    let (new_email, _) = normalize_email(new_email_raw).map_err(|e| e.to_string())?;
    Ok(ChangeEmailRequest {
        current_password,
        new_email,
    })
}

pub type WebSyncDispatchFn =
    Box<dyn FnMut(SyncWorkerCommand, mpsc::Sender<SyncWorkerEvent>, Option<egui::Context>)>;

pub(crate) struct BackgroundSyncWorker {
    cmd_tx: Option<mpsc::Sender<SyncWorkerCommand>>,
    pub(crate) evt_tx: mpsc::Sender<SyncWorkerEvent>,
    evt_rx: mpsc::Receiver<SyncWorkerEvent>,
    repaint_ctx: Arc<Mutex<Option<egui::Context>>>,
    busy: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pending_outcome: Option<(bool, SyncCycleOutcome)>,
    startup_sync_queued: bool,
    last_attempt_ui_ms: f64,
    next_retry_delay_ms: f64,
    last_discovery_sent: Option<(f64, LocalDiscoveryReport)>,
    last_discovery_check_ui_ms: Option<f64>,
    last_discovery_state_key: Option<(u64, bool, usize)>,
}

impl BackgroundSyncWorker {
    pub(crate) fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<SyncWorkerCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<SyncWorkerEvent>();
        let repaint_ctx: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
        let busy = Arc::new(AtomicBool::new(false));

        let ctx_slot = Arc::clone(&repaint_ctx);
        let busy_flag = Arc::clone(&busy);
        let thread_evt_tx = evt_tx.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let handle = std::thread::Builder::new()
            .name("drawcraft-sync".to_string())
            .spawn(move || worker_loop(cmd_rx, thread_evt_tx, ctx_slot, busy_flag))
            .ok();
        #[cfg(target_arch = "wasm32")]
        let handle = {
            let _ = (cmd_rx, thread_evt_tx, ctx_slot, busy_flag);
            None
        };

        Self {
            cmd_tx: Some(cmd_tx),
            evt_tx,
            evt_rx,
            repaint_ctx,
            busy,
            handle,
            pending_outcome: None,
            startup_sync_queued: false,
            last_attempt_ui_ms: -100_000.0,
            next_retry_delay_ms: 15_000.0,
            last_discovery_sent: None,
            last_discovery_check_ui_ms: None,
            last_discovery_state_key: None,
        }
    }

    pub(crate) fn send(&self, ctx: Option<&egui::Context>, cmd: SyncWorkerCommand) -> bool {
        if let Some(ctx) = ctx
            && let Ok(mut guard) = self.repaint_ctx.lock()
        {
            *guard = Some(ctx.clone());
        }
        self.cmd_tx.as_ref().is_some_and(|tx| tx.send(cmd).is_ok())
    }

    pub(crate) fn shutdown(&mut self) {
        self.cmd_tx = None;
        if !self.busy.load(Ordering::Relaxed)
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
    }
}

pub(crate) fn dispatch_command(
    app: &mut DrawCraftApp,
    ctx: Option<&egui::Context>,
    cmd: SyncWorkerCommand,
) -> bool {
    if let Some(web_dispatch) = app.hooks.web_sync_dispatch.as_mut() {
        web_dispatch(cmd, app.sync_worker.evt_tx.clone(), ctx.cloned());
        return true;
    }
    app.sync_worker.send(ctx, cmd)
}

pub(crate) fn maybe_report_local_discovery(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    now_ui_ms: f64,
) {
    if !now_ui_ms.is_finite()
        || !app.session.is_sync_persistence_enabled()
        || app.session.needs_initial_nickname()
    {
        return;
    }
    let state_key = (
        app.session.revision,
        app.session.is_viewing_cloud(),
        app.session.accessible_local_profiles().len(),
    );
    let periodic_due = app
        .sync_worker
        .last_discovery_sent
        .as_ref()
        .is_none_or(|(prev_ms, _)| now_ui_ms - *prev_ms >= DISCOVERY_REPORT_INTERVAL_MS);
    let change_due = app.sync_worker.last_discovery_state_key != Some(state_key)
        && app
            .sync_worker
            .last_discovery_check_ui_ms
            .is_none_or(|prev_check| now_ui_ms - prev_check >= 2_000.0);
    if !periodic_due && !change_due {
        return;
    }

    app.sync_worker.last_discovery_check_ui_ms = Some(now_ui_ms);
    app.sync_worker.last_discovery_state_key = Some(state_key);

    let report = app.session.local_discovery_report();
    if report.client_id.trim().is_empty() {
        return;
    }
    let should_send = match &app.sync_worker.last_discovery_sent {
        None => true,
        Some((prev_ms, prev_rep)) => {
            prev_rep != &report || (now_ui_ms - *prev_ms) >= DISCOVERY_REPORT_INTERVAL_MS
        }
    };
    if !should_send {
        return;
    }
    let server_url = app.session.server_url().to_string();
    app.sync_worker.last_discovery_sent = Some((now_ui_ms, report.clone()));
    let _ = dispatch_command(
        app,
        Some(ctx),
        SyncWorkerCommand::ReportLocalDiscovery { server_url, report },
    );
}

pub(crate) fn trigger_manual_sync(app: &mut DrawCraftApp, ctx: &egui::Context) {
    if app.view.account.pending_conflict.is_some() {
        return;
    }
    let Some(acct) = app.session.active_account_session().cloned() else {
        return;
    };
    if app.session.is_sync_in_flight() {
        return;
    }
    let offline_edits_pending = app.session.offline_edits_pending();
    let Some(batch) = app.session.prepare_sync_batch() else {
        return;
    };
    app.session.set_sync_in_flight(true);
    app.sync_worker.last_attempt_ui_ms = ctx.input(|i| i.time * 1000.0);
    if !dispatch_command(
        app,
        Some(ctx),
        SyncWorkerCommand::SyncNow {
            session: acct,
            batch: Box::new(batch),
            offline_edits_pending,
        },
    ) {
        app.view.account.clear_transient_auth_status();
        app.session
            .record_sync_failure(false, "Background sync worker unavailable.".to_string());
    }
}

pub(crate) fn trigger_conflict_keep_server(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let Some(acct) = app.session.active_account_session().cloned() else {
        return;
    };
    app.view.account.conflict_resolving = true;
    app.view.account.error_message = None;
    app.session.set_sync_in_flight(true);
    if !dispatch_command(
        app,
        Some(ctx),
        SyncWorkerCommand::ResolveConflictKeepServer { session: acct },
    ) {
        app.view.account.conflict_resolving = false;
        app.session
            .record_sync_failure(false, "Background sync worker unavailable.".to_string());
    }
}

pub(crate) fn trigger_conflict_keep_local(app: &mut DrawCraftApp, ctx: &egui::Context) {
    let Some(acct) = app.session.active_account_session().cloned() else {
        return;
    };
    let Some(batch) = app.session.prepare_force_local_sync_batch() else {
        return;
    };
    app.view.account.conflict_resolving = true;
    app.view.account.error_message = None;
    app.session.set_sync_in_flight(true);
    if !dispatch_command(
        app,
        Some(ctx),
        SyncWorkerCommand::ResolveConflictKeepLocal {
            session: acct,
            batch: Box::new(batch),
        },
    ) {
        app.view.account.conflict_resolving = false;
        app.session
            .record_sync_failure(false, "Background sync worker unavailable.".to_string());
    }
}

pub(crate) fn poll_and_tick(app: &mut DrawCraftApp, ctx: &egui::Context, now_ui_ms: f64) {
    drain_worker_events(app, ctx);
    let can_mutate_docs = app.session.live_stroke.is_none() && app.session.live_eraser.is_none();
    if can_mutate_docs {
        maybe_report_local_discovery(app, ctx, now_ui_ms);
    }
    if can_mutate_docs
        && let Some((replace_server, outcome)) = app.sync_worker.pending_outcome.take()
    {
        let prev_theme = app.session.prefs.theme;
        let prev_active_title = app.session.doc.title.clone();
        let prev_active_cover = app.session.doc.cover;
        let prev_rev = app.session.revision;
        let apply_res = if replace_server {
            app.session.apply_keep_server_version(outcome)
        } else {
            app.session.apply_sync_outcome(outcome)
        };
        if let Err(err) = apply_res {
            app.view.account.sign_out_after_sync = false;
            app.view.account.conflict_resolving = false;
            app.session.record_sync_failure(false, err);
            app.view.account.clear_transient_auth_status();
        } else {
            app.sync_worker.next_retry_delay_ms = 15_000.0;
            app.view.account.clear_transient_auth_status();
            app.view.account.pending_conflict = None;
            app.view.account.conflict_resolving = false;
            if app.view.active_modal == Some(crate::modal::ActiveModal::SyncConflict) {
                crate::modal::close(app);
            }
            if app.session.revision != prev_rev {
                app.mesh_cache.clear();
            }
            if app.session.prefs.theme != prev_theme {
                crate::theme::apply_visuals(ctx, app.session.prefs.theme);
            }
            if app.session.doc.title != prev_active_title
                || app.session.doc.cover != prev_active_cover
            {
                app.view.notebook_title_draft = app.session.doc.title.clone();
                app.view.notebook_cover_draft = app.session.doc.cover;
            }
            if app.view.account.sign_out_after_sync {
                app.view.account.sign_out_after_sync = false;
                if app.session.cloud_pending_changes_count() == 0 {
                    complete_sign_out(app, ctx);
                } else {
                    app.view.account.error_message = Some(
                        "Some pending changes could not be uploaded yet. Review them before signing out."
                            .to_string(),
                    );
                }
            }
        }
    }

    if !app.session.is_signed_in()
        || !app.session.is_viewing_cloud()
        || app.session.is_sync_in_flight()
        || app.sync_worker.pending_outcome.is_some()
        || app.view.account.pending_conflict.is_some()
        || app.session.needs_initial_nickname()
        || !can_mutate_docs
        || !app.session.is_sync_persistence_enabled()
    {
        return;
    }

    let elapsed_since_last = now_ui_ms - app.sync_worker.last_attempt_ui_ms;
    let should_startup_sync = !app.sync_worker.startup_sync_queued;
    let has_dirty_ready = app.session.last_dirty_at_ms().is_some_and(|dirty_wall_ms| {
        now_unix_ms() - dirty_wall_ms >= AUTO_SYNC_DEBOUNCE_MS as i64
            && elapsed_since_last
                >= app
                    .sync_worker
                    .next_retry_delay_ms
                    .min(AUTO_SYNC_DEBOUNCE_MS)
    });
    if app.session.last_dirty_at_ms().is_some() && !has_dirty_ready {
        ctx.request_repaint_after(Duration::from_secs(4));
    }
    let periodic_pull =
        elapsed_since_last >= AUTO_SYNC_IDLE_PULL_MS.max(app.sync_worker.next_retry_delay_ms);

    if should_startup_sync || has_dirty_ready || periodic_pull {
        app.sync_worker.startup_sync_queued = true;
        trigger_manual_sync(app, ctx);
    }
}

fn drain_worker_events(app: &mut DrawCraftApp, ctx: &egui::Context) {
    while let Ok(evt) = app.sync_worker.evt_rx.try_recv() {
        apply_worker_event(app, ctx, evt);
    }
}

/// Applies an authenticated cloud session with `mode` and triggers the initial sync cycle.
pub(crate) fn complete_authenticated_switch(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    auth: ActiveAccountSession,
    mode: AuthWorkspaceMode,
) {
    let label = auth.user.primary_label();
    app.view.account.apply_auth_ok(&auth.server_url, &label);
    app.view.account.sync_identity_from_session(Some(&auth));
    app.view.profile_return_screen = crate::AppScreen::Home;
    app.session.switch_to_authenticated_library(auth, mode);
    app.sync_worker.next_retry_delay_ms = 15_000.0;
    app.sync_worker.startup_sync_queued = true;
    crate::sync_view_to_active_notebook(app);
    trigger_manual_sync(app, ctx);
}

/// Completes sign-out to the local profile, notifying the server in the background.
pub(crate) fn complete_sign_out(app: &mut DrawCraftApp, ctx: &egui::Context) {
    if let Some(sess) = app.session.active_account_session().cloned() {
        let _ = dispatch_command(app, Some(ctx), SyncWorkerCommand::Logout { session: sess });
    }
    app.session.sign_out_to_local_library();
    app.view.profile_return_screen = crate::AppScreen::Home;
    app.view
        .account
        .reset_after_sign_out(app.session.server_url());
    crate::sync_view_to_active_notebook(app);
}

pub(crate) fn apply_worker_event(
    app: &mut DrawCraftApp,
    ctx: &egui::Context,
    evt: SyncWorkerEvent,
) {
    match evt {
        SyncWorkerEvent::HealthOk { server_url, info } => {
            app.session.set_network_offline_state(false);
            app.view.account.apply_health_ok(server_url, info);
        }
        SyncWorkerEvent::HealthErr {
            server_url,
            message,
        } => {
            app.view.account.apply_health_err(server_url, message);
        }
        SyncWorkerEvent::AuthOk { auth, mode } => {
            app.session.set_network_offline_state(false);
            complete_authenticated_switch(app, ctx, auth, mode);
        }
        SyncWorkerEvent::AuthErr { message } => {
            app.view.account.apply_auth_err(message);
        }
        SyncWorkerEvent::PasswordResetRequired {
            identifier,
            prefill_code,
            message,
        } => {
            app.view
                .account
                .enter_password_reset_flow(identifier, prefill_code, message);
        }
        SyncWorkerEvent::ProfileUpdated { updated_session } => {
            let next_username = updated_session.user.username.clone();
            let next_display = updated_session.user.display_name.clone();
            app.session.update_active_session_tokens(updated_session);
            app.view
                .account
                .apply_profile_updated(&next_username, &next_display);
        }
        SyncWorkerEvent::ProfileUpdateErr { message } => {
            app.view.account.apply_profile_update_err(message);
        }
        SyncWorkerEvent::PasswordChanged { updated_session } => {
            app.session.update_active_session_tokens(updated_session);
            app.view.account.apply_password_changed();
        }
        SyncWorkerEvent::PasswordChangeErr { message } => {
            app.view.account.apply_password_change_err(message);
        }
        SyncWorkerEvent::EmailChanged { updated_session } => {
            let next_email = updated_session.user.email.clone();
            app.session.update_active_session_tokens(updated_session);
            app.view.account.apply_email_changed(next_email.as_deref());
        }
        SyncWorkerEvent::EmailChangeErr { message } => {
            app.view.account.apply_email_change_err(message);
        }
        SyncWorkerEvent::SyncOk { outcome } => {
            app.sync_worker.pending_outcome = Some((false, *outcome));
        }
        SyncWorkerEvent::ConflictDecisionRequired { preview } => {
            app.session.set_sync_in_flight(false);
            app.view.account.sign_out_after_sync = false;
            app.view.account.clear_transient_auth_status();
            app.view.account.conflict_resolving = false;
            app.view.account.pending_conflict = Some(preview);
            crate::modal::open(app, crate::modal::ActiveModal::SyncConflict);
        }
        SyncWorkerEvent::ConflictResolvedKeepServer { outcome } => {
            app.sync_worker.pending_outcome = Some((true, *outcome));
        }
        SyncWorkerEvent::SyncErr {
            offline,
            unauthorized,
            message,
        } => {
            app.view.account.sign_out_after_sync = false;
            app.view.account.conflict_resolving = false;
            app.view.account.clear_transient_auth_status();
            app.session.record_sync_failure(offline, message);
            let next_backoff = if unauthorized {
                120_000.0
            } else {
                (app.sync_worker.next_retry_delay_ms * 2.0).clamp(15_000.0, 120_000.0)
            };
            app.sync_worker.next_retry_delay_ms = next_backoff;
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker_loop(
    cmd_rx: mpsc::Receiver<SyncWorkerCommand>,
    evt_tx: mpsc::Sender<SyncWorkerEvent>,
    repaint_ctx: Arc<Mutex<Option<egui::Context>>>,
    busy: Arc<AtomicBool>,
) {
    let client = SyncHttpClient::new();
    while let Ok(cmd) = cmd_rx.recv() {
        busy.store(true, Ordering::Relaxed);
        let maybe_event = execute_worker_command(&client, cmd);
        busy.store(false, Ordering::Relaxed);

        if let Some(evt) = maybe_event {
            if evt_tx.send(evt).is_err() {
                break;
            }
            if let Ok(guard) = repaint_ctx.lock()
                && let Some(ref ctx) = *guard
            {
                ctx.request_repaint();
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn execute_worker_command(
    client: &SyncHttpClient,
    cmd: SyncWorkerCommand,
) -> Option<SyncWorkerEvent> {
    match cmd {
        SyncWorkerCommand::TestConnection { server_url } => {
            let norm = drawcraft_sync_protocol::normalize_server_url(&server_url)
                .unwrap_or_else(|_| server_url.clone());
            match client.health(&norm) {
                Ok(info) => Some(SyncWorkerEvent::HealthOk {
                    server_url: norm,
                    info,
                }),
                Err(err) => Some(SyncWorkerEvent::HealthErr {
                    server_url: norm,
                    message: friendly_error(&err),
                }),
            }
        }
        SyncWorkerCommand::ReportLocalDiscovery { server_url, report } => {
            let norm =
                drawcraft_sync_protocol::normalize_server_url(&server_url).unwrap_or(server_url);
            let _ = client.report_local_discovery(&norm, &report);
            None
        }
        SyncWorkerCommand::Register {
            server_url,
            email,
            username,
            password,
            display_name,
            device_name,
            mode,
        } => {
            let req = match build_register_request(
                &email,
                username.as_deref(),
                password,
                display_name,
                &device_name,
            ) {
                Ok(req) => req,
                Err(message) => return Some(SyncWorkerEvent::AuthErr { message }),
            };
            match client.register(&server_url, &req, &device_name) {
                Ok(auth) => Some(SyncWorkerEvent::AuthOk { auth, mode }),
                Err(err) => Some(SyncWorkerEvent::AuthErr {
                    message: friendly_auth_error(&err),
                }),
            }
        }
        SyncWorkerCommand::Login {
            server_url,
            identifier,
            password,
            device_name,
            mode,
        } => {
            let submitted_password = password.clone();
            let req = match build_login_request(&identifier, password, &device_name) {
                Ok(req) => req,
                Err(message) => return Some(SyncWorkerEvent::AuthErr { message }),
            };
            match client.login(&server_url, &req, &device_name) {
                Ok(auth) => Some(SyncWorkerEvent::AuthOk { auth, mode }),
                Err(SyncError::Server { code, message, .. })
                    if code == ERR_CODE_PASSWORD_CHANGE_REQUIRED =>
                {
                    Some(SyncWorkerEvent::PasswordResetRequired {
                        identifier,
                        prefill_code: Some(submitted_password.trim().to_string()),
                        message,
                    })
                }
                Err(SyncError::Server { code, message, .. })
                    if code == ERR_CODE_PASSWORD_RESET_REQUIRED =>
                {
                    Some(SyncWorkerEvent::PasswordResetRequired {
                        identifier,
                        prefill_code: None,
                        message,
                    })
                }
                Err(err) => Some(SyncWorkerEvent::AuthErr {
                    message: friendly_auth_error(&err),
                }),
            }
        }
        SyncWorkerCommand::CompletePasswordReset {
            server_url,
            identifier,
            recovery_code,
            new_password,
            device_name,
            mode,
        } => {
            let req = match build_complete_password_reset_request(
                &identifier,
                &recovery_code,
                new_password,
                &device_name,
            ) {
                Ok(req) => req,
                Err(message) => return Some(SyncWorkerEvent::AuthErr { message }),
            };
            match client.complete_password_reset(&server_url, &req, &device_name) {
                Ok(auth) => Some(SyncWorkerEvent::AuthOk { auth, mode }),
                Err(err) => Some(SyncWorkerEvent::AuthErr {
                    message: friendly_auth_error(&err),
                }),
            }
        }
        SyncWorkerCommand::UpdateProfile {
            mut session,
            username,
            display_name,
        } => {
            let req =
                match build_update_profile_request(username.as_deref(), display_name.as_deref()) {
                    Ok(req) => req,
                    Err(message) => return Some(SyncWorkerEvent::ProfileUpdateErr { message }),
                };
            match client.update_profile_with_refresh(&mut session, &req) {
                Ok((profile, _)) => {
                    session.user = profile;
                    Some(SyncWorkerEvent::ProfileUpdated {
                        updated_session: session,
                    })
                }
                Err(err) => Some(SyncWorkerEvent::ProfileUpdateErr {
                    message: friendly_error(&err),
                }),
            }
        }
        SyncWorkerCommand::ChangePassword {
            mut session,
            current_password,
            new_password,
        } => {
            let req = match build_change_password_request(current_password, new_password) {
                Ok(req) => req,
                Err(message) => return Some(SyncWorkerEvent::PasswordChangeErr { message }),
            };
            match client.change_password_with_refresh(&mut session, &req) {
                Ok((profile, _)) => {
                    session.user = profile;
                    Some(SyncWorkerEvent::PasswordChanged {
                        updated_session: session,
                    })
                }
                Err(err) => Some(SyncWorkerEvent::PasswordChangeErr {
                    message: friendly_error(&err),
                }),
            }
        }
        SyncWorkerCommand::ChangeEmail {
            mut session,
            new_email,
            current_password,
        } => {
            let req = match build_change_email_request(&new_email, current_password) {
                Ok(req) => req,
                Err(message) => return Some(SyncWorkerEvent::EmailChangeErr { message }),
            };
            match client.change_email_with_refresh(&mut session, &req) {
                Ok((profile, _)) => {
                    session.user = profile;
                    Some(SyncWorkerEvent::EmailChanged {
                        updated_session: session,
                    })
                }
                Err(err) => Some(SyncWorkerEvent::EmailChangeErr {
                    message: friendly_error(&err),
                }),
            }
        }
        SyncWorkerCommand::SyncNow {
            mut session,
            batch,
            offline_edits_pending,
        } => Some(exec_guarded_sync_now(
            client,
            &mut session,
            &batch,
            offline_edits_pending,
        )),
        SyncWorkerCommand::ResolveConflictKeepServer { mut session } => {
            match client.pull_with_refresh(&mut session, 0) {
                Ok((pull_resp, refreshed)) => Some(SyncWorkerEvent::ConflictResolvedKeepServer {
                    outcome: Box::new(outcome_from_full_pull(
                        pull_resp,
                        refreshed.then_some(session),
                    )),
                }),
                Err(err) => Some(sync_err_event(&err)),
            }
        }
        SyncWorkerCommand::ResolveConflictKeepLocal { mut session, batch } => {
            match run_sync_cycle_keep_local(client, &mut session, &batch) {
                Ok(outcome) => Some(SyncWorkerEvent::SyncOk {
                    outcome: Box::new(outcome),
                }),
                Err(err) => Some(sync_err_event(&err)),
            }
        }
        SyncWorkerCommand::Logout { session } => {
            let _ = client.logout(&session);
            None
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn exec_guarded_sync_now(
    client: &SyncHttpClient,
    session: &mut ActiveAccountSession,
    batch: &SyncBatch,
    offline_edits_pending: bool,
) -> SyncWorkerEvent {
    let initial_since = if offline_edits_pending || batch.client_cursor == 0 {
        0
    } else {
        batch.client_cursor
    };
    let mut pull_resp = match client.pull_with_refresh(session, initial_since) {
        Ok((resp, _)) => resp,
        Err(err) => return sync_err_event(&err),
    };

    let acct_name = session.user.primary_label();
    let acct_email = session.user.email.clone().unwrap_or_default();
    if let Some(mut preview) = build_conflict_preview_if_needed(
        &acct_name,
        &acct_email,
        offline_edits_pending,
        batch,
        &pull_resp,
    ) {
        if initial_since > 0
            && let Ok((full_pull, _)) = client.pull_with_refresh(session, 0)
            && let Some(full_preview) = build_conflict_preview_if_needed(
                &acct_name,
                &acct_email,
                offline_edits_pending,
                batch,
                &full_pull,
            )
        {
            pull_resp = full_pull;
            preview = full_preview;
        }
        let _ = pull_resp;
        return SyncWorkerEvent::ConflictDecisionRequired { preview };
    }

    match run_sync_cycle(client, session, batch) {
        Ok(outcome) => SyncWorkerEvent::SyncOk {
            outcome: Box::new(outcome),
        },
        Err(err) => sync_err_event(&err),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_err_event(err: &SyncError) -> SyncWorkerEvent {
    SyncWorkerEvent::SyncErr {
        offline: matches!(err, SyncError::Network(_)),
        unauthorized: matches!(err, SyncError::Unauthorized(_)),
        message: friendly_error(err),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn friendly_auth_error(err: &SyncError) -> String {
    match err {
        SyncError::Unauthorized(msg) if !msg.trim().is_empty() => msg.clone(),
        _ => friendly_error(err),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn friendly_error(err: &SyncError) -> String {
    match err {
        SyncError::Network(_) => {
            "Server is currently unreachable. Verify it is running and reachable over your LAN or VPN."
                .to_string()
        }
        SyncError::Unauthorized(_) => {
            "Session expired or unauthorized. Please sign in again.".to_string()
        }
        SyncError::Server { message, .. } => message.clone(),
        SyncError::Protocol(p) => p.to_string(),
        SyncError::Io(msg) | SyncError::InvalidResponse(msg) => msg.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sync_error_clears_stale_syncing_status_banner() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        app.view.account.status_message =
            Some("Signed in as @ada. Syncing notebooks...".to_string());

        apply_worker_event(
            &mut app,
            &ctx,
            SyncWorkerEvent::SyncErr {
                offline: false,
                unauthorized: false,
                message: "Server error".to_string(),
            },
        );

        assert!(app.view.account.status_message.is_none());
        assert_eq!(
            app.session.sync_status_summary().last_error.as_deref(),
            Some("Server error")
        );
    }

    #[test]
    fn conflict_decision_required_opens_sync_conflict_modal_and_pauses_sync() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::new(
            drawcraft_engine::Session::open_web_cache(None),
            crate::PlatformHooks::default(),
        );
        let auth = ActiveAccountSession {
            server_url: "http://127.0.0.1:8787".to_string(),
            user: drawcraft_sync_protocol::UserProfile {
                user_id: "u-existing".to_string(),
                email: Some("ada@example.com".to_string()),
                username: "ada".to_string(),
                display_name: "Ada".to_string(),
                created_at_ms: 1000,
                must_change_password: false,
            },
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            access_expires_at_ms: 99_999,
            refresh_expires_at_ms: 99_999,
            device_name: "TestPC".to_string(),
            last_auth_ms: 1000,
        };
        app.session
            .switch_to_authenticated_library(auth, AuthWorkspaceMode::SwitchOnly);
        app.session.set_sync_in_flight(true);

        apply_worker_event(
            &mut app,
            &ctx,
            SyncWorkerEvent::ConflictDecisionRequired {
                preview: SyncConflictPreview {
                    account_name: "Ada".to_string(),
                    account_email: "ada@example.com".to_string(),
                    server_notebook_count: 2,
                    local_notebook_count: 1,
                    server_cursor: 4,
                    local_pending_changes: 1,
                    local_dirty_titles: vec!["Offline Draft".to_string()],
                    server_notebook_titles: vec!["Cloud Note".to_string()],
                    offline_edits_detected: true,
                },
            },
        );

        assert!(!app.session.is_sync_in_flight());
        assert_eq!(
            app.view.active_modal,
            Some(crate::modal::ActiveModal::SyncConflict)
        );
        assert!(app.view.account.pending_conflict.is_some());
    }

    #[test]
    fn local_discovery_skips_active_stroke_and_throttles_per_frame_polling() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let ctx = egui::Context::default();
        let dispatch_count = Arc::new(AtomicUsize::new(0));
        let dispatch_clone = Arc::clone(&dispatch_count);

        let mut session = drawcraft_engine::Session::open_web_cache(None);
        session
            .execute("profile.completeFirstAccess", &json_nick("Tester"))
            .unwrap();
        let hooks = crate::PlatformHooks {
            web_sync_dispatch: Some(Box::new(move |cmd, _tx, _ctx| {
                if matches!(cmd, SyncWorkerCommand::ReportLocalDiscovery { .. }) {
                    dispatch_clone.fetch_add(1, Ordering::Relaxed);
                }
            })),
            ..crate::PlatformHooks::default()
        };
        let mut app = DrawCraftApp::new(session, hooks);

        // Active drawing must completely defer discovery checks.
        app.session
            .begin_live_stroke(drawcraft_doc::StrokeSample::from_xy_pressure(
                10.0, 20.0, 0.5,
            ));
        poll_and_tick(&mut app, &ctx, 100.0);
        assert_eq!(dispatch_count.load(Ordering::Relaxed), 0);
        assert_eq!(app.sync_worker.last_discovery_check_ui_ms, None);

        // First idle frame sends initial report.
        app.session.cancel_live_stroke();
        poll_and_tick(&mut app, &ctx, 100.0);
        assert_eq!(dispatch_count.load(Ordering::Relaxed), 1);
        assert_eq!(app.sync_worker.last_discovery_check_ui_ms, Some(100.0));

        // High-frequency idle frames within 2s must not re-poll discovery even on revision bump.
        app.session
            .execute("notebook.create", &serde_json::json!({ "title": "Second" }))
            .unwrap();
        for t in [116.0, 250.0, 1000.0, 1900.0] {
            poll_and_tick(&mut app, &ctx, t);
        }
        assert_eq!(app.sync_worker.last_discovery_check_ui_ms, Some(100.0));
        assert_eq!(dispatch_count.load(Ordering::Relaxed), 1);

        // After 2s debounce with changed state, discovery refreshes cleanly and dispatches updated count.
        poll_and_tick(&mut app, &ctx, 2200.0);
        assert_eq!(app.sync_worker.last_discovery_check_ui_ms, Some(2200.0));
        assert_eq!(dispatch_count.load(Ordering::Relaxed), 2);
    }

    fn json_nick(nick: &str) -> serde_json::Value {
        serde_json::json!({ "nickname": nick })
    }
}
