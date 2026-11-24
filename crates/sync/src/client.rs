#[cfg(not(target_arch = "wasm32"))]
use drawcraft_sync_protocol::{
    AuthResponse, ChangeEmailRequest, ChangePasswordRequest, CheckEmailRequest, CheckEmailResponse,
    CompletePasswordResetRequest, LogoutRequest, PruneToLocalResponse, RefreshRequest,
    UpdateProfileRequest, UserProfile, now_unix_ms,
};
#[cfg(target_arch = "wasm32")]
use drawcraft_sync_protocol::{
    ChangeEmailRequest, ChangePasswordRequest, CheckEmailResponse, CompletePasswordResetRequest,
    PruneToLocalResponse, UpdateProfileRequest, UserProfile,
};
use drawcraft_sync_protocol::{
    LocalDiscoveryReport, LocalDiscoverySummary, LoginRequest, PruneToLocalRequest,
    RegisterRequest, ServerInfoResponse, SyncPullResponse, SyncPushRequest, SyncPushResponse,
};

#[cfg(not(target_arch = "wasm32"))]
use drawcraft_sync_protocol::{ApiErrorBody, normalize_server_url};
#[cfg(not(target_arch = "wasm32"))]
use serde::Serialize;
#[cfg(not(target_arch = "wasm32"))]
use serde::de::DeserializeOwned;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use ureq::Agent;
#[cfg(not(target_arch = "wasm32"))]
use ureq::http::Response;

use crate::types::{ActiveAccountSession, SyncError};

#[cfg(not(target_arch = "wasm32"))]
const MAX_SYNC_RESPONSE_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(not(target_arch = "wasm32"))]
const MAX_ERROR_BODY_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub struct SyncHttpClient {
    #[cfg(not(target_arch = "wasm32"))]
    agent: Agent,
}

impl Default for SyncHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SyncHttpClient {
    pub fn new() -> Self {
        let cfg = ureq::config::Config::builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .build();
        Self {
            agent: Agent::new_with_config(cfg),
        }
    }

    pub fn health(&self, server_url: &str) -> Result<ServerInfoResponse, SyncError> {
        let base = normalize_server_url(server_url)?;
        let resp = self
            .agent
            .get(&format!("{base}/api/v1/health"))
            .call()
            .map_err(map_transport_error)?;
        decode_json_response(resp)
    }

    pub fn report_local_discovery(
        &self,
        server_url: &str,
        report: &LocalDiscoveryReport,
    ) -> Result<LocalDiscoverySummary, SyncError> {
        let base = normalize_server_url(server_url)?;
        self.post_json(
            &format!("{base}/api/v1/discovery/local-accounts"),
            None,
            report,
        )
    }

    pub fn check_email(
        &self,
        server_url: &str,
        email: &str,
    ) -> Result<CheckEmailResponse, SyncError> {
        let base = normalize_server_url(server_url)?;
        let req = CheckEmailRequest {
            email: email.to_string(),
        };
        self.post_json(&format!("{base}/api/v1/auth/check-email"), None, &req)
    }

    pub fn change_email_with_refresh(
        &self,
        session: &mut ActiveAccountSession,
        req: &ChangeEmailRequest,
    ) -> Result<(UserProfile, bool), SyncError> {
        let url = format!("{}/api/v1/auth/change-email", session.server_url);
        match self.post_json::<ChangeEmailRequest, UserProfile>(
            &url,
            Some(&session.access_token),
            req,
        ) {
            Ok(profile) => Ok((profile, false)),
            Err(SyncError::Unauthorized(_)) => {
                *session = self.refresh(
                    &session.server_url,
                    &session.refresh_token,
                    &session.device_name,
                )?;
                let profile = self.post_json(&url, Some(&session.access_token), req)?;
                Ok((profile, true))
            }
            Err(err) => Err(err),
        }
    }

    pub fn register(
        &self,
        server_url: &str,
        req: &RegisterRequest,
        device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        let base = normalize_server_url(server_url)?;
        let auth: AuthResponse =
            self.post_json(&format!("{base}/api/v1/auth/register"), None, req)?;
        Ok(session_from_auth(base, auth, device_name))
    }

    pub fn login(
        &self,
        server_url: &str,
        req: &LoginRequest,
        device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        let base = normalize_server_url(server_url)?;
        let auth: AuthResponse = self.post_json(&format!("{base}/api/v1/auth/login"), None, req)?;
        Ok(session_from_auth(base, auth, device_name))
    }

    pub fn complete_password_reset(
        &self,
        server_url: &str,
        req: &CompletePasswordResetRequest,
        device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        let base = normalize_server_url(server_url)?;
        let auth: AuthResponse = self.post_json(
            &format!("{base}/api/v1/auth/complete-password-reset"),
            None,
            req,
        )?;
        Ok(session_from_auth(base, auth, device_name))
    }

    pub fn update_profile_with_refresh(
        &self,
        session: &mut ActiveAccountSession,
        req: &UpdateProfileRequest,
    ) -> Result<(UserProfile, bool), SyncError> {
        let url = format!("{}/api/v1/auth/update-profile", session.server_url);
        match self.post_json::<UpdateProfileRequest, UserProfile>(
            &url,
            Some(&session.access_token),
            req,
        ) {
            Ok(profile) => Ok((profile, false)),
            Err(SyncError::Unauthorized(_)) => {
                *session = self.refresh(
                    &session.server_url,
                    &session.refresh_token,
                    &session.device_name,
                )?;
                let profile = self.post_json(&url, Some(&session.access_token), req)?;
                Ok((profile, true))
            }
            Err(err) => Err(err),
        }
    }

    pub fn change_password_with_refresh(
        &self,
        session: &mut ActiveAccountSession,
        req: &ChangePasswordRequest,
    ) -> Result<(UserProfile, bool), SyncError> {
        let url = format!("{}/api/v1/auth/change-password", session.server_url);
        match self.post_json::<ChangePasswordRequest, UserProfile>(
            &url,
            Some(&session.access_token),
            req,
        ) {
            Ok(profile) => Ok((profile, false)),
            Err(SyncError::Unauthorized(_)) => {
                *session = self.refresh(
                    &session.server_url,
                    &session.refresh_token,
                    &session.device_name,
                )?;
                let profile = self.post_json(&url, Some(&session.access_token), req)?;
                Ok((profile, true))
            }
            Err(err) => Err(err),
        }
    }

    pub fn refresh(
        &self,
        server_url: &str,
        refresh_token: &str,
        device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        let base = normalize_server_url(server_url)?;
        let body = RefreshRequest {
            refresh_token: refresh_token.to_string(),
        };
        let auth: AuthResponse =
            self.post_json(&format!("{base}/api/v1/auth/refresh"), None, &body)?;
        Ok(session_from_auth(base, auth, device_name))
    }

    pub fn logout(&self, session: &ActiveAccountSession) -> Result<(), SyncError> {
        let url = format!("{}/api/v1/auth/logout", session.server_url);
        let body = LogoutRequest {
            refresh_token: Some(session.refresh_token.clone()),
        };
        let mut resp = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", session.access_token))
            .send_json(&body)
            .map_err(map_transport_error)?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) || status == 401 {
            return Ok(());
        }
        Err(extract_error_body(status, &mut resp))
    }

    pub fn pull(
        &self,
        session: &ActiveAccountSession,
        since_cursor: u64,
    ) -> Result<SyncPullResponse, SyncError> {
        let url = format!(
            "{}/api/v1/sync/pull?since={since_cursor}",
            session.server_url
        );
        let resp = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", session.access_token))
            .call()
            .map_err(map_transport_error)?;
        decode_json_response(resp)
    }

    pub fn push(
        &self,
        session: &ActiveAccountSession,
        req: &SyncPushRequest,
    ) -> Result<SyncPushResponse, SyncError> {
        let url = format!("{}/api/v1/sync/push", session.server_url);
        self.post_json(&url, Some(&session.access_token), req)
    }

    pub fn pull_with_refresh(
        &self,
        session: &mut ActiveAccountSession,
        since_cursor: u64,
    ) -> Result<(SyncPullResponse, bool), SyncError> {
        match self.pull(session, since_cursor) {
            Ok(resp) => Ok((resp, false)),
            Err(SyncError::Unauthorized(_)) => {
                *session = self.refresh(
                    &session.server_url,
                    &session.refresh_token,
                    &session.device_name,
                )?;
                let resp = self.pull(session, since_cursor)?;
                Ok((resp, true))
            }
            Err(err) => Err(err),
        }
    }

    pub fn push_with_refresh(
        &self,
        session: &mut ActiveAccountSession,
        req: &SyncPushRequest,
    ) -> Result<(SyncPushResponse, bool), SyncError> {
        match self.push(session, req) {
            Ok(resp) => Ok((resp, false)),
            Err(SyncError::Unauthorized(_)) => {
                *session = self.refresh(
                    &session.server_url,
                    &session.refresh_token,
                    &session.device_name,
                )?;
                let resp = self.push(session, req)?;
                Ok((resp, true))
            }
            Err(err) => Err(err),
        }
    }

    pub fn prune_to_local(
        &self,
        session: &ActiveAccountSession,
        req: &PruneToLocalRequest,
    ) -> Result<PruneToLocalResponse, SyncError> {
        let url = format!("{}/api/v1/sync/prune-to-local", session.server_url);
        self.post_json(&url, Some(&session.access_token), req)
    }

    pub fn prune_to_local_with_refresh(
        &self,
        session: &mut ActiveAccountSession,
        req: &PruneToLocalRequest,
    ) -> Result<(PruneToLocalResponse, bool), SyncError> {
        match self.prune_to_local(session, req) {
            Ok(resp) => Ok((resp, false)),
            Err(SyncError::Unauthorized(_)) => {
                *session = self.refresh(
                    &session.server_url,
                    &session.refresh_token,
                    &session.device_name,
                )?;
                let resp = self.prune_to_local(session, req)?;
                Ok((resp, true))
            }
            Err(err) => Err(err),
        }
    }

    fn post_json<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        url: &str,
        bearer: Option<&str>,
        payload: &Req,
    ) -> Result<Resp, SyncError> {
        let mut req = self.agent.post(url);
        if let Some(tok) = bearer {
            req = req.header("Authorization", &format!("Bearer {tok}"));
        }
        let resp = req.send_json(payload).map_err(map_transport_error)?;
        decode_json_response(resp)
    }
}

#[cfg(target_arch = "wasm32")]
impl SyncHttpClient {
    pub fn new() -> Self {
        Self {}
    }

    pub fn health(&self, _server_url: &str) -> Result<ServerInfoResponse, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn check_email(
        &self,
        _server_url: &str,
        _email: &str,
    ) -> Result<CheckEmailResponse, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn change_email_with_refresh(
        &self,
        _session: &mut ActiveAccountSession,
        _req: &ChangeEmailRequest,
    ) -> Result<(UserProfile, bool), SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn register(
        &self,
        _server_url: &str,
        _req: &RegisterRequest,
        _device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn login(
        &self,
        _server_url: &str,
        _req: &LoginRequest,
        _device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn complete_password_reset(
        &self,
        _server_url: &str,
        _req: &CompletePasswordResetRequest,
        _device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn update_profile_with_refresh(
        &self,
        _session: &mut ActiveAccountSession,
        _req: &UpdateProfileRequest,
    ) -> Result<(UserProfile, bool), SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn change_password_with_refresh(
        &self,
        _session: &mut ActiveAccountSession,
        _req: &ChangePasswordRequest,
    ) -> Result<(UserProfile, bool), SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn refresh(
        &self,
        _server_url: &str,
        _refresh_token: &str,
        _device_name: &str,
    ) -> Result<ActiveAccountSession, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn logout(&self, _session: &ActiveAccountSession) -> Result<(), SyncError> {
        Ok(())
    }

    pub fn pull(
        &self,
        _session: &ActiveAccountSession,
        _since_cursor: u64,
    ) -> Result<SyncPullResponse, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn push(
        &self,
        _session: &ActiveAccountSession,
        _req: &SyncPushRequest,
    ) -> Result<SyncPushResponse, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn pull_with_refresh(
        &self,
        _session: &mut ActiveAccountSession,
        _since_cursor: u64,
    ) -> Result<(SyncPullResponse, bool), SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn push_with_refresh(
        &self,
        _session: &mut ActiveAccountSession,
        _req: &SyncPushRequest,
    ) -> Result<(SyncPushResponse, bool), SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn report_local_discovery(
        &self,
        _server_url: &str,
        _report: &LocalDiscoveryReport,
    ) -> Result<LocalDiscoverySummary, SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }

    pub fn prune_to_local_with_refresh(
        &self,
        _session: &mut ActiveAccountSession,
        _req: &PruneToLocalRequest,
    ) -> Result<(PruneToLocalResponse, bool), SyncError> {
        Err(SyncError::Network(
            "Sync is unavailable in web demo".to_string(),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn map_transport_error(err: ureq::Error) -> SyncError {
    SyncError::Network(err.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_json_response<T: DeserializeOwned>(
    mut resp: Response<ureq::Body>,
) -> Result<T, SyncError> {
    let status = resp.status().as_u16();
    if (200..300).contains(&status) {
        return resp
            .body_mut()
            .with_config()
            .limit(MAX_SYNC_RESPONSE_BYTES)
            .read_json::<T>()
            .map_err(|e| SyncError::InvalidResponse(format!("malformed JSON payload: {e}")));
    }
    Err(extract_error_body(status, &mut resp))
}

#[cfg(not(target_arch = "wasm32"))]
fn extract_error_body(status: u16, resp: &mut Response<ureq::Body>) -> SyncError {
    let raw = resp
        .body_mut()
        .with_config()
        .limit(MAX_ERROR_BODY_BYTES)
        .read_to_string()
        .unwrap_or_default();
    let parsed = serde_json::from_str::<ApiErrorBody>(&raw);
    let (code, message) = match parsed {
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
