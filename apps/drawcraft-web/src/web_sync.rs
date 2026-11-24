use std::sync::mpsc;

use drawcraft_sync::{
    ActiveAccountSession, SyncBatch, SyncCycleDriver, SyncCycleOutcome, SyncError,
    build_conflict_preview_if_needed, build_prune_to_local_request, outcome_from_full_pull,
};
use drawcraft_sync_protocol::{
    ApiErrorBody, AuthResponse, ChangeEmailRequest, ERR_CODE_PASSWORD_CHANGE_REQUIRED,
    ERR_CODE_PASSWORD_RESET_REQUIRED, LocalDiscoverySummary, LogoutRequest, PruneToLocalRequest,
    PruneToLocalResponse, RefreshRequest, ServerInfoResponse, SyncPullResponse, SyncPushRequest,
    SyncPushResponse, UserProfile, normalize_server_url, now_unix_ms,
};
use drawcraft_ui::sync_worker::{
    SyncWorkerCommand, SyncWorkerEvent, build_change_email_request, build_change_password_request,
    build_complete_password_reset_request, build_login_request, build_register_request,
    build_update_profile_request,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::{JsFuture, spawn_local};

pub fn dispatch_sync_command(
    cmd: SyncWorkerCommand,
    evt_tx: mpsc::Sender<SyncWorkerEvent>,
    ctx: Option<egui::Context>,
) {
    spawn_local(async move {
        let evt = execute_async_command(cmd).await;
        if let Some(event) = evt {
            let _ = evt_tx.send(event);
            if let Some(egui_ctx) = ctx {
                egui_ctx.request_repaint();
            }
        }
    });
}

async fn execute_async_command(cmd: SyncWorkerCommand) -> Option<SyncWorkerEvent> {
    match cmd {
        SyncWorkerCommand::TestConnection { server_url } => {
            let norm = normalize_server_url(&server_url).unwrap_or_else(|_| server_url.clone());
            match web_health(&norm).await {
                Ok(info) => Some(SyncWorkerEvent::HealthOk {
                    server_url: norm,
                    info,
                }),
                Err(err) => Some(SyncWorkerEvent::HealthErr {
                    server_url: norm,
                    message: friendly_web_error(&err),
                }),
            }
        }
        SyncWorkerCommand::ReportLocalDiscovery { server_url, report } => {
            let norm = normalize_server_url(&server_url).unwrap_or(server_url);
            let _: Result<LocalDiscoverySummary, SyncError> = fetch_json(
                &format!("{norm}/api/v1/discovery/local-accounts"),
                "POST",
                None,
                Some(&report),
            )
            .await;
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
            match web_auth("/api/v1/auth/register", &server_url, &req, &device_name).await {
                Ok(auth) => Some(SyncWorkerEvent::AuthOk { auth, mode }),
                Err(err) => Some(SyncWorkerEvent::AuthErr {
                    message: friendly_web_auth_error(&err),
                }),
            }
        }
        SyncWorkerCommand::Login {
            server_url,
            identifier,
            password,
            device_name,
            mode,
        } => Some(exec_web_login(&server_url, identifier, password, &device_name, mode).await),
        SyncWorkerCommand::CompletePasswordReset {
            server_url,
            identifier,
            recovery_code,
            new_password,
            device_name,
            mode,
        } => Some(
            exec_web_complete_password_reset(
                &server_url,
                &identifier,
                &recovery_code,
                new_password,
                &device_name,
                mode,
            )
            .await,
        ),
        SyncWorkerCommand::UpdateProfile {
            session,
            username,
            display_name,
        } => Some(exec_web_update_profile(session, username, display_name).await),
        SyncWorkerCommand::ChangePassword {
            session,
            current_password,
            new_password,
        } => Some(exec_web_change_password(session, current_password, new_password).await),
        SyncWorkerCommand::ChangeEmail {
            mut session,
            new_email,
            current_password,
        } => {
            let req = match build_change_email_request(&new_email, current_password) {
                Ok(req) => req,
                Err(message) => return Some(SyncWorkerEvent::EmailChangeErr { message }),
            };
            match web_change_email_with_refresh(&mut session, &req).await {
                Ok(profile) => {
                    session.user = profile;
                    Some(SyncWorkerEvent::EmailChanged {
                        updated_session: session,
                    })
                }
                Err(err) => Some(SyncWorkerEvent::EmailChangeErr {
                    message: friendly_web_error(&err),
                }),
            }
        }
        SyncWorkerCommand::SyncNow {
            mut session,
            batch,
            offline_edits_pending,
        } => Some(exec_web_guarded_sync_now(&mut session, &batch, offline_edits_pending).await),
        SyncWorkerCommand::ResolveConflictKeepServer { mut session } => {
            match pull_with_refresh(&mut session, 0).await {
                Ok((pull_resp, refreshed)) => Some(SyncWorkerEvent::ConflictResolvedKeepServer {
                    outcome: Box::new(outcome_from_full_pull(
                        pull_resp,
                        refreshed.then_some(session),
                    )),
                }),
                Err(err) => Some(web_sync_err_event(&err)),
            }
        }
        SyncWorkerCommand::ResolveConflictKeepLocal { mut session, batch } => {
            match run_web_sync_cycle_keep_local(&mut session, &batch).await {
                Ok(outcome) => Some(SyncWorkerEvent::SyncOk {
                    outcome: Box::new(outcome),
                }),
                Err(err) => Some(web_sync_err_event(&err)),
            }
        }
        SyncWorkerCommand::Logout { session } => {
            let url = format!("{}/api/v1/auth/logout", session.server_url);
            let body = LogoutRequest {
                refresh_token: Some(session.refresh_token),
            };
            let _: Result<serde_json::Value, SyncError> =
                fetch_json(&url, "POST", Some(&session.access_token), Some(&body)).await;
            None
        }
    }
}

async fn exec_web_login(
    server_url: &str,
    identifier: String,
    password: String,
    device_name: &str,
    mode: drawcraft_engine::AuthWorkspaceMode,
) -> SyncWorkerEvent {
    let submitted_password = password.clone();
    let req = match build_login_request(&identifier, password, device_name) {
        Ok(req) => req,
        Err(message) => return SyncWorkerEvent::AuthErr { message },
    };
    match web_auth("/api/v1/auth/login", server_url, &req, device_name).await {
        Ok(auth) => SyncWorkerEvent::AuthOk { auth, mode },
        Err(SyncError::Server { code, message, .. })
            if code == ERR_CODE_PASSWORD_CHANGE_REQUIRED =>
        {
            SyncWorkerEvent::PasswordResetRequired {
                identifier,
                prefill_code: Some(submitted_password.trim().to_string()),
                message,
            }
        }
        Err(SyncError::Server { code, message, .. })
            if code == ERR_CODE_PASSWORD_RESET_REQUIRED =>
        {
            SyncWorkerEvent::PasswordResetRequired {
                identifier,
                prefill_code: None,
                message,
            }
        }
        Err(err) => SyncWorkerEvent::AuthErr {
            message: friendly_web_auth_error(&err),
        },
    }
}

async fn exec_web_complete_password_reset(
    server_url: &str,
    identifier: &str,
    recovery_code: &str,
    new_password: String,
    device_name: &str,
    mode: drawcraft_engine::AuthWorkspaceMode,
) -> SyncWorkerEvent {
    let req = match build_complete_password_reset_request(
        identifier,
        recovery_code,
        new_password,
        device_name,
    ) {
        Ok(req) => req,
        Err(message) => return SyncWorkerEvent::AuthErr { message },
    };
    match web_auth(
        "/api/v1/auth/complete-password-reset",
        server_url,
        &req,
        device_name,
    )
    .await
    {
        Ok(auth) => SyncWorkerEvent::AuthOk { auth, mode },
        Err(err) => SyncWorkerEvent::AuthErr {
            message: friendly_web_auth_error(&err),
        },
    }
}

async fn exec_web_update_profile(
    mut session: ActiveAccountSession,
    username: Option<String>,
    display_name: Option<String>,
) -> SyncWorkerEvent {
    let req = match build_update_profile_request(username.as_deref(), display_name.as_deref()) {
        Ok(req) => req,
        Err(message) => return SyncWorkerEvent::ProfileUpdateErr { message },
    };
    match web_post_profile_with_refresh(&mut session, "/api/v1/auth/update-profile", &req).await {
        Ok(profile) => {
            session.user = profile;
            SyncWorkerEvent::ProfileUpdated {
                updated_session: session,
            }
        }
        Err(err) => SyncWorkerEvent::ProfileUpdateErr {
            message: friendly_web_error(&err),
        },
    }
}

async fn exec_web_change_password(
    mut session: ActiveAccountSession,
    current_password: String,
    new_password: String,
) -> SyncWorkerEvent {
    let req = match build_change_password_request(current_password, new_password) {
        Ok(req) => req,
        Err(message) => return SyncWorkerEvent::PasswordChangeErr { message },
    };
    match web_post_profile_with_refresh(&mut session, "/api/v1/auth/change-password", &req).await {
        Ok(profile) => {
            session.user = profile;
            SyncWorkerEvent::PasswordChanged {
                updated_session: session,
            }
        }
        Err(err) => SyncWorkerEvent::PasswordChangeErr {
            message: friendly_web_error(&err),
        },
    }
}

async fn web_health(server_url: &str) -> Result<ServerInfoResponse, SyncError> {
    let base = normalize_server_url(server_url)?;
    fetch_json::<(), ServerInfoResponse>(&format!("{base}/api/v1/health"), "GET", None, None).await
}

fn web_sync_err_event(err: &SyncError) -> SyncWorkerEvent {
    SyncWorkerEvent::SyncErr {
        offline: matches!(err, SyncError::Network(_)),
        unauthorized: matches!(err, SyncError::Unauthorized(_)),
        message: friendly_web_error(err),
    }
}

async fn exec_web_guarded_sync_now(
    session: &mut ActiveAccountSession,
    batch: &SyncBatch,
    offline_edits_pending: bool,
) -> SyncWorkerEvent {
    let initial_since = if offline_edits_pending || batch.client_cursor == 0 {
        0
    } else {
        batch.client_cursor
    };
    let pull_resp = match pull_with_refresh(session, initial_since).await {
        Ok((resp, _)) => resp,
        Err(err) => return web_sync_err_event(&err),
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
            && let Ok((full_pull, _)) = pull_with_refresh(session, 0).await
            && let Some(full_preview) = build_conflict_preview_if_needed(
                &acct_name,
                &acct_email,
                offline_edits_pending,
                batch,
                &full_pull,
            )
        {
            preview = full_preview;
        }
        return SyncWorkerEvent::ConflictDecisionRequired { preview };
    }

    match run_web_sync_cycle(session, batch).await {
        Ok(outcome) => SyncWorkerEvent::SyncOk {
            outcome: Box::new(outcome),
        },
        Err(err) => web_sync_err_event(&err),
    }
}

async fn run_web_sync_cycle_keep_local(
    session: &mut ActiveAccountSession,
    batch: &SyncBatch,
) -> Result<SyncCycleOutcome, SyncError> {
    let mut forced = batch.clone();
    forced.force_overwrite = true;
    let initial_access = session.access_token.clone();
    let mut driver = SyncCycleDriver::new(&forced);

    while let Some(req) = driver.next_push_request(&forced) {
        let resp = push_with_refresh(session, &req).await?;
        driver.record_push_chunk_response(resp, &forced);
    }

    let prune_req = build_prune_to_local_request(&forced);
    let _ = prune_with_refresh(session, &prune_req).await?;
    let (pull_resp, refreshed_on_pull) = pull_with_refresh(session, 0).await?;
    let token_changed = refreshed_on_pull || session.access_token != initial_access;
    Ok(driver.finish(pull_resp, token_changed.then(|| session.clone())))
}

async fn prune_with_refresh(
    session: &mut ActiveAccountSession,
    req: &PruneToLocalRequest,
) -> Result<PruneToLocalResponse, SyncError> {
    let url = format!("{}/api/v1/sync/prune-to-local", session.server_url);
    match fetch_json(&url, "POST", Some(&session.access_token), Some(req)).await {
        Ok(resp) => Ok(resp),
        Err(SyncError::Unauthorized(_)) => {
            *session = web_refresh(
                &session.server_url,
                &session.refresh_token,
                &session.device_name,
            )
            .await?;
            fetch_json(&url, "POST", Some(&session.access_token), Some(req)).await
        }
        Err(err) => Err(err),
    }
}

async fn web_post_profile_with_refresh<Req: Serialize>(
    session: &mut ActiveAccountSession,
    endpoint: &str,
    req: &Req,
) -> Result<UserProfile, SyncError> {
    let url = format!("{}{endpoint}", session.server_url);
    match fetch_json::<Req, UserProfile>(&url, "POST", Some(&session.access_token), Some(req)).await
    {
        Ok(profile) => Ok(profile),
        Err(SyncError::Unauthorized(_)) => {
            *session = web_refresh(
                &session.server_url,
                &session.refresh_token,
                &session.device_name,
            )
            .await?;
            fetch_json(&url, "POST", Some(&session.access_token), Some(req)).await
        }
        Err(err) => Err(err),
    }
}

async fn web_change_email_with_refresh(
    session: &mut ActiveAccountSession,
    req: &ChangeEmailRequest,
) -> Result<UserProfile, SyncError> {
    let url = format!("{}/api/v1/auth/change-email", session.server_url);
    match fetch_json::<ChangeEmailRequest, UserProfile>(
        &url,
        "POST",
        Some(&session.access_token),
        Some(req),
    )
    .await
    {
        Ok(profile) => Ok(profile),
        Err(SyncError::Unauthorized(_)) => {
            *session = web_refresh(
                &session.server_url,
                &session.refresh_token,
                &session.device_name,
            )
            .await?;
            fetch_json(&url, "POST", Some(&session.access_token), Some(req)).await
        }
        Err(err) => Err(err),
    }
}

async fn web_auth<Req: Serialize>(
    endpoint: &str,
    server_url: &str,
    req: &Req,
    device_name: &str,
) -> Result<ActiveAccountSession, SyncError> {
    let base = normalize_server_url(server_url)?;
    let auth: AuthResponse =
        fetch_json(&format!("{base}{endpoint}"), "POST", None, Some(req)).await?;
    Ok(session_from_auth(base, auth, device_name))
}

async fn web_refresh(
    server_url: &str,
    refresh_token: &str,
    device_name: &str,
) -> Result<ActiveAccountSession, SyncError> {
    let base = normalize_server_url(server_url)?;
    let body = RefreshRequest {
        refresh_token: refresh_token.to_string(),
    };
    let auth: AuthResponse = fetch_json(
        &format!("{base}/api/v1/auth/refresh"),
        "POST",
        None,
        Some(&body),
    )
    .await?;
    Ok(session_from_auth(base, auth, device_name))
}

async fn run_web_sync_cycle(
    session: &mut ActiveAccountSession,
    batch: &SyncBatch,
) -> Result<SyncCycleOutcome, SyncError> {
    let initial_access = session.access_token.clone();
    let mut driver = SyncCycleDriver::new(batch);

    while let Some(req) = driver.next_push_request(batch) {
        let resp = push_with_refresh(session, &req).await?;
        driver.record_push_chunk_response(resp, batch);
    }

    let (pull_resp, refreshed_on_pull) = pull_with_refresh(session, batch.client_cursor).await?;
    let token_changed = refreshed_on_pull || session.access_token != initial_access;
    Ok(driver.finish(pull_resp, token_changed.then(|| session.clone())))
}

async fn push_with_refresh(
    session: &mut ActiveAccountSession,
    req: &SyncPushRequest,
) -> Result<SyncPushResponse, SyncError> {
    let url = format!("{}/api/v1/sync/push", session.server_url);
    match fetch_json(&url, "POST", Some(&session.access_token), Some(req)).await {
        Ok(resp) => Ok(resp),
        Err(SyncError::Unauthorized(_)) => {
            *session = web_refresh(
                &session.server_url,
                &session.refresh_token,
                &session.device_name,
            )
            .await?;
            fetch_json(&url, "POST", Some(&session.access_token), Some(req)).await
        }
        Err(err) => Err(err),
    }
}

async fn pull_with_refresh(
    session: &mut ActiveAccountSession,
    since_cursor: u64,
) -> Result<(SyncPullResponse, bool), SyncError> {
    let url = format!(
        "{}/api/v1/sync/pull?since={since_cursor}",
        session.server_url
    );
    match fetch_json::<(), SyncPullResponse>(&url, "GET", Some(&session.access_token), None).await {
        Ok(resp) => Ok((resp, false)),
        Err(SyncError::Unauthorized(_)) => {
            *session = web_refresh(
                &session.server_url,
                &session.refresh_token,
                &session.device_name,
            )
            .await?;
            let resp =
                fetch_json::<(), SyncPullResponse>(&url, "GET", Some(&session.access_token), None)
                    .await?;
            Ok((resp, true))
        }
        Err(err) => Err(err),
    }
}

async fn fetch_json<Req: Serialize, Resp: DeserializeOwned>(
    url: &str,
    method: &str,
    bearer: Option<&str>,
    body: Option<&Req>,
) -> Result<Resp, SyncError> {
    let window = web_sys::window()
        .ok_or_else(|| SyncError::Network("browser window unavailable".to_string()))?;
    let headers = web_sys::Headers::new()
        .map_err(|_| SyncError::Network("failed creating HTTP headers".to_string()))?;
    let _ = headers.set("Accept", "application/json");
    if body.is_some() {
        let _ = headers.set("Content-Type", "application/json");
    }
    if let Some(tok) = bearer {
        let _ = headers.set("Authorization", &format!("Bearer {tok}"));
    }

    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_mode(web_sys::RequestMode::Cors);
    init.set_headers(&headers);
    if let Some(payload) = body {
        let raw_json = serde_json::to_string(payload)
            .map_err(|e| SyncError::InvalidResponse(format!("JSON encode failed: {e}")))?;
        init.set_body(&JsValue::from_str(&raw_json));
    }

    let request = web_sys::Request::new_with_str_and_init(url, &init)
        .map_err(|e| SyncError::Network(js_error_message(&e)))?;
    let resp_val = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| SyncError::Network(js_error_message(&e)))?;
    let resp = resp_val
        .dyn_into::<web_sys::Response>()
        .map_err(|_| SyncError::InvalidResponse("invalid fetch Response object".to_string()))?;

    let status = resp.status();
    let text_promise = resp
        .text()
        .map_err(|e| SyncError::Network(js_error_message(&e)))?;
    let raw_text = JsFuture::from(text_promise)
        .await
        .map_err(|e| SyncError::Network(js_error_message(&e)))?
        .as_string()
        .unwrap_or_default();

    if (200..300).contains(&status) {
        return serde_json::from_str::<Resp>(&raw_text)
            .map_err(|e| SyncError::InvalidResponse(format!("malformed JSON payload: {e}")));
    }

    Err(parse_error_response(status, &raw_text))
}

fn parse_error_response(status: u16, raw: &str) -> SyncError {
    let (code, message) = match serde_json::from_str::<ApiErrorBody>(raw) {
        Ok(err) => (err.code, err.message),
        Err(_) if status == 413 => (
            "payload_too_large".to_string(),
            "Upload batch exceeds the server request size limit.".to_string(),
        ),
        Err(_) => (
            format!("http_{status}"),
            format!("Server returned HTTP status {status}."),
        ),
    };
    if status == 401 {
        SyncError::Unauthorized(message)
    } else {
        SyncError::Server {
            status,
            code,
            message,
        }
    }
}

fn session_from_auth(
    server_url: String,
    auth: AuthResponse,
    device_name: &str,
) -> ActiveAccountSession {
    ActiveAccountSession {
        server_url,
        user: auth.user,
        access_token: auth.access_token,
        refresh_token: auth.refresh_token,
        access_expires_at_ms: auth.expires_at_ms,
        refresh_expires_at_ms: auth.refresh_expires_at_ms,
        device_name: device_name.to_string(),
        last_auth_ms: now_unix_ms(),
    }
}

fn js_error_message(err: &JsValue) -> String {
    err.as_string()
        .or_else(|| {
            js_sys::Reflect::get(err, &JsValue::from_str("message"))
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| "browser network request failed".to_string())
}

fn friendly_web_auth_error(err: &SyncError) -> String {
    match err {
        SyncError::Unauthorized(msg) if !msg.trim().is_empty() => msg.clone(),
        _ => friendly_web_error(err),
    }
}

fn friendly_web_error(err: &SyncError) -> String {
    match err {
        SyncError::Network(_) => {
            "Server is currently unreachable. Verify it is running, reachable, and allows browser requests."
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
