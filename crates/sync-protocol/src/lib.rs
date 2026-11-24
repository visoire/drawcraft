//! L2 shared wire protocol, DTOs, and input validation for DrawCraft account and sync.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use drawcraft_doc::{Document, InkStroke, NotebookPage, QuantizedSample, UserStudioConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_SERVER_PORT: u16 = 8787;
pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8787";

pub const MIN_USERNAME_LEN: usize = 3;
pub const MAX_USERNAME_LEN: usize = 32;
pub const MAX_EMAIL_LEN: usize = 254;
pub const MIN_PASSWORD_LEN: usize = 8;
pub const MAX_PASSWORD_LEN: usize = 128;
pub const MAX_DISPLAY_NAME_LEN: usize = 64;
pub const MAX_DEVICE_NAME_LEN: usize = 64;
pub const MAX_SERVER_URL_LEN: usize = 512;
pub const MAX_NOTEBOOK_ID_LEN: usize = 64;
pub const MAX_FOLDER_ID_LEN: usize = 64;
pub const MAX_PUSH_BATCH_SIZE: usize = 64;

pub const ERR_CODE_PASSWORD_RESET_REQUIRED: &str = "password_reset_required";
pub const ERR_CODE_PASSWORD_CHANGE_REQUIRED: &str = "password_change_required";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("invalid server URL: {0}")]
    InvalidServerUrl(String),
    #[error("invalid username: {0}")]
    InvalidUsername(String),
    #[error("invalid email address: {0}")]
    InvalidEmail(String),
    #[error("invalid password: {0}")]
    InvalidPassword(String),
    #[error("invalid notebook ID: {0}")]
    InvalidNotebookId(String),
    #[error("invalid folder ID: {0}")]
    InvalidFolderId(String),
    #[error("invalid sync payload: {0}")]
    InvalidPayload(String),
}

/// Validates and normalizes a DrawCraft server URL (e.g. `100.64.0.12:8787` or `https://notes.example.ts.net`).
pub fn normalize_server_url(raw: &str) -> Result<String, ProtocolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ProtocolError::InvalidServerUrl(
            "server URL cannot be empty".to_string(),
        ));
    }
    if trimmed.len() > MAX_SERVER_URL_LEN {
        return Err(ProtocolError::InvalidServerUrl(
            "server URL exceeds 512 characters".to_string(),
        ));
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '?' | '#'))
    {
        return Err(ProtocolError::InvalidServerUrl(
            "server URL must not contain spaces, control characters, queries, or fragments"
                .to_string(),
        ));
    }

    let with_scheme = if let Some(idx) = trimmed.find("://") {
        let scheme = trimmed[..idx].to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(ProtocolError::InvalidServerUrl(
                "only http:// and https:// server URLs are supported".to_string(),
            ));
        }
        format!("{scheme}://{}", &trimmed[idx + 3..])
    } else {
        format!("http://{trimmed}")
    };

    let normalized = with_scheme.trim_end_matches('/').to_string();
    let after_scheme = normalized.split_once("://").map_or("", |(_, rest)| rest);
    let authority = after_scheme.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(ProtocolError::InvalidServerUrl(
            "server URL must include a valid host without embedded credentials".to_string(),
        ));
    }

    validate_authority(authority)?;
    Ok(normalized)
}

fn validate_authority(authority: &str) -> Result<(), ProtocolError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((addr, after)) = rest.split_once(']') else {
            return Err(ProtocolError::InvalidServerUrl(
                "unclosed IPv6 bracket in server host".to_string(),
            ));
        };
        if addr.is_empty()
            || !addr
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            return Err(ProtocolError::InvalidServerUrl(
                "invalid IPv6 host in server URL".to_string(),
            ));
        }
        if !after.is_empty() {
            let Some(port_str) = after.strip_prefix(':') else {
                return Err(ProtocolError::InvalidServerUrl(
                    "invalid port separator after IPv6 host".to_string(),
                ));
            };
            parse_valid_port(port_str)?;
        }
        return Ok(());
    }

    if let Some((host, port_str)) = authority.rsplit_once(':') {
        if host.is_empty() || host.contains(':') {
            return Err(ProtocolError::InvalidServerUrl(
                "invalid host or unbracketed IPv6 address".to_string(),
            ));
        }
        validate_host_chars(host)?;
        parse_valid_port(port_str)?;
    } else {
        validate_host_chars(authority)?;
    }
    Ok(())
}

fn validate_host_chars(host: &str) -> Result<(), ProtocolError> {
    if host.is_empty()
        || host.starts_with('.')
        || host.ends_with('.')
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return Err(ProtocolError::InvalidServerUrl(
            "server host contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn parse_valid_port(port_str: &str) -> Result<u16, ProtocolError> {
    let port: u16 = port_str.parse().map_err(|_| {
        ProtocolError::InvalidServerUrl(
            "server port must be a number between 1 and 65535".to_string(),
        )
    })?;
    if port == 0 {
        return Err(ProtocolError::InvalidServerUrl(
            "server port must be between 1 and 65535".to_string(),
        ));
    }
    Ok(port)
}

/// Validates and normalizes a username, returning `(trimmed_display, lowercase_key)`.
pub fn normalize_username(raw: &str) -> Result<(String, String), ProtocolError> {
    let trimmed = raw.trim();
    if trimmed.len() < MIN_USERNAME_LEN || trimmed.len() > MAX_USERNAME_LEN {
        return Err(ProtocolError::InvalidUsername(format!(
            "username must be between {MIN_USERNAME_LEN} and {MAX_USERNAME_LEN} characters"
        )));
    }
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return Err(ProtocolError::InvalidUsername(
            "username cannot be empty".to_string(),
        ));
    };
    if !first.is_ascii_alphanumeric() {
        return Err(ProtocolError::InvalidUsername(
            "username must start with an ASCII letter or digit".to_string(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(ProtocolError::InvalidUsername(
            "username may only contain ASCII letters, numbers, '.', '_', or '-'".to_string(),
        ));
    }
    Ok((trimmed.to_string(), trimmed.to_ascii_lowercase()))
}

/// Sanitizes an optional user display name, falling back to `fallback_username`.
pub fn sanitize_display_name(raw: &str, fallback_username: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DISPLAY_NAME_LEN)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        fallback_username.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Validates a plaintext password against basic length and control-character rules.
pub fn validate_password(raw: &str) -> Result<(), ProtocolError> {
    if raw.chars().any(|c| c.is_control()) {
        return Err(ProtocolError::InvalidPassword(
            "password must not contain control characters".to_string(),
        ));
    }
    let char_count = raw.chars().count();
    if char_count < MIN_PASSWORD_LEN || raw.len() > MAX_PASSWORD_LEN {
        return Err(ProtocolError::InvalidPassword(format!(
            "password must be between {MIN_PASSWORD_LEN} and {MAX_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

/// Sanitizes a client device label (e.g. `"DESKTOP-ALEX"`).
pub fn sanitize_device_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_NAME_LEN)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Windows Desktop".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Validates and normalizes an email address, returning `(trimmed_display, lowercase_key)`.
pub fn normalize_email(raw: &str) -> Result<(String, String), ProtocolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_EMAIL_LEN {
        return Err(ProtocolError::InvalidEmail(
            "email address must be between 3 and 254 characters".to_string(),
        ));
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(ProtocolError::InvalidEmail(
            "email address must not contain spaces or control characters".to_string(),
        ));
    }
    let Some((local, domain)) = trimmed.split_once('@') else {
        return Err(ProtocolError::InvalidEmail(
            "email address must include `@` and a domain".to_string(),
        ));
    };
    if local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || domain.contains('@')
    {
        return Err(ProtocolError::InvalidEmail(
            "invalid email local part".to_string(),
        ));
    }
    if domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.contains("..")
        || !domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    {
        return Err(ProtocolError::InvalidEmail(
            "invalid email domain".to_string(),
        ));
    }
    Ok((trimmed.to_string(), trimmed.to_ascii_lowercase()))
}

/// Derives a sanitized handle from an email address for display/legacy compatibility.
pub fn derive_username_from_email(email: &str) -> String {
    let local = email.trim().split('@').next().unwrap_or("user");
    let mut cleaned: String = local
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_USERNAME_LEN)
        .collect();
    if !cleaned
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        cleaned.insert(0, 'u');
    }
    while cleaned.len() < MIN_USERNAME_LEN {
        cleaned.push('0');
    }
    cleaned.truncate(MAX_USERNAME_LEN);
    cleaned
}

/// Validates a stable notebook identifier (`UUID` or slug).
pub fn validate_notebook_id(raw: &str) -> Result<String, ProtocolError> {
    validate_ascii_entity_id(raw, MAX_NOTEBOOK_ID_LEN).map_err(ProtocolError::InvalidNotebookId)
}

/// Validates a stable folder identifier (`UUID` or slug).
pub fn validate_folder_id(raw: &str) -> Result<String, ProtocolError> {
    validate_ascii_entity_id(raw, MAX_FOLDER_ID_LEN).map_err(ProtocolError::InvalidFolderId)
}

fn validate_ascii_entity_id(raw: &str, max_len: usize) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > max_len {
        return Err(format!("ID must be between 1 and {max_len} characters"));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err("ID must contain only ASCII letters, digits, '_' or '-'".to_string());
    }
    Ok(trimmed.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
}

/// Privacy-preserving summary of unsynced local accounts detected by or reported to the server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDiscoverySummary {
    pub unsynced_local_accounts: usize,
    pub total_local_notebooks: usize,
}

/// Client report carrying only aggregate local account and notebook counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalDiscoveryReport {
    pub client_id: String,
    pub unsynced_local_accounts: usize,
    pub total_local_notebooks: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfoResponse {
    pub status: String,
    pub service: String,
    pub version: String,
    pub protocol_version: u32,
    pub registration_open: bool,
    pub storage_engine: String,
    pub max_notebook_bytes: usize,
    #[serde(default)]
    pub local_discovery: LocalDiscoverySummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub username: String,
    pub display_name: String,
    pub created_at_ms: i64,
    #[serde(default)]
    pub must_change_password: bool,
}

impl UserProfile {
    /// Returns the primary human-facing identifier (`email` if set, otherwise `@username`).
    pub fn primary_label(&self) -> String {
        match self.email.as_deref() {
            Some(email) if !email.trim().is_empty() => email.to_string(),
            _ => format!("@{}", self.username),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub device_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckEmailRequest {
    pub email: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckEmailResponse {
    pub exists: bool,
    #[serde(default)]
    pub registration_open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEmailRequest {
    pub current_password: String,
    pub new_email: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProfileRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletePasswordResetRequest {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub username: String,
    pub recovery_code: String,
    pub new_password: String,
    #[serde(default)]
    pub device_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogoutRequest {
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
    pub refresh_expires_at_ms: i64,
    pub user: UserProfile,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteNotebookRecord {
    pub notebook_id: String,
    pub version: u64,
    pub cursor: u64,
    pub deleted: bool,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub deleted_at_ms: Option<i64>,
    pub title: String,
    pub content_hash: String,
    #[serde(default)]
    pub body_hash: String,
    #[serde(default)]
    pub meta_hash: String,
    #[serde(default)]
    pub meta_updated_at_ms: i64,
    #[serde(default)]
    pub document: Option<Document>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFolderRecord {
    pub folder_id: String,
    pub version: u64,
    pub cursor: u64,
    pub path: String,
    #[serde(default)]
    pub color_hex: Option<String>,
    pub deleted: bool,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub deleted_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemotePrefsRecord {
    pub version: u64,
    pub updated_at_ms: i64,
    pub content_hash: String,
    pub prefs: UserStudioConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncPullResponse {
    pub server_cursor: u64,
    pub user: UserProfile,
    pub notebooks: Vec<RemoteNotebookRecord>,
    #[serde(default)]
    pub folders: Vec<RemoteFolderRecord>,
    #[serde(default)]
    pub studio_prefs: Option<RemotePrefsRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalNotebookChange {
    pub notebook_id: String,
    pub base_version: u64,
    pub deleted: bool,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub deleted_at_ms: Option<i64>,
    pub content_hash: String,
    #[serde(default)]
    pub body_hash: String,
    #[serde(default)]
    pub meta_hash: String,
    #[serde(default)]
    pub meta_updated_at_ms: i64,
    #[serde(default)]
    pub document: Option<Document>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalFolderChange {
    pub folder_id: String,
    pub base_version: u64,
    pub path: String,
    #[serde(default)]
    pub color_hex: Option<String>,
    pub deleted: bool,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub deleted_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocalPrefsPush {
    pub base_version: u64,
    pub content_hash: String,
    pub prefs: UserStudioConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncPushRequest {
    pub client_cursor: u64,
    pub device_name: String,
    pub changes: Vec<LocalNotebookChange>,
    #[serde(default)]
    pub folders: Vec<LocalFolderChange>,
    #[serde(default)]
    pub studio_prefs: Option<LocalPrefsPush>,
    #[serde(default)]
    pub force_overwrite: bool,
}

/// Authoritative prune request issued after keeping the local version in conflict resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneToLocalRequest {
    #[serde(default)]
    pub keep_notebook_ids: Vec<String>,
    #[serde(default)]
    pub keep_folder_ids: Vec<String>,
    #[serde(default)]
    pub device_name: String,
}

/// Response returned after pruning unchosen server notebooks and folders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneToLocalResponse {
    pub server_cursor: u64,
    pub pruned_notebooks: usize,
    pub pruned_folders: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedNotebookSync {
    pub notebook_id: String,
    pub new_version: u64,
    pub cursor: u64,
    pub deleted: bool,
    pub content_hash: String,
    #[serde(default)]
    pub body_hash: String,
    #[serde(default)]
    pub meta_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedFolderSync {
    pub folder_id: String,
    pub new_version: u64,
    pub cursor: u64,
    pub deleted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConflictNotebookSync {
    pub notebook_id: String,
    pub server_record: RemoteNotebookRecord,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncPushResponse {
    pub server_cursor: u64,
    pub accepted: Vec<AcceptedNotebookSync>,
    pub conflicts: Vec<ConflictNotebookSync>,
    #[serde(default)]
    pub accepted_folders: Vec<AcceptedFolderSync>,
    #[serde(default)]
    pub folder_conflicts: Vec<RemoteFolderRecord>,
    pub prefs_version: u64,
    #[serde(default)]
    pub prefs_conflict: Option<RemotePrefsRecord>,
}

/// Encodes arbitrary bytes as lowercase hexadecimal.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

/// Computes the lowercase hex SHA-256 digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    bytes_to_hex(&digest)
}

fn hash_len_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn hash_quantized_sample(hasher: &mut Sha256, q: QuantizedSample) {
    hasher.update(q.x_centipx.to_le_bytes());
    hasher.update(q.y_centipx.to_le_bytes());
    hasher.update(q.pressure_q16.to_le_bytes());
    hasher.update(q.tilt_x_cdeg.to_le_bytes());
    hasher.update(q.tilt_y_cdeg.to_le_bytes());
    hasher.update(q.rot_cdeg.to_le_bytes());
    hasher.update(q.time_ms.to_le_bytes());
}

fn hash_stroke_into(hasher: &mut Sha256, stroke: &InkStroke) {
    hasher.update(stroke.id.0.to_le_bytes());
    let clean_pen = stroke.pen.sanitized();
    if let Ok(pen_bytes) = serde_json::to_vec(&clean_pen) {
        hash_len_bytes(hasher, &pen_bytes);
    }
    hasher.update((stroke.smoothed_points.len() as u64).to_le_bytes());
    for &sample in stroke.smoothed_points.iter() {
        hash_quantized_sample(hasher, sample.quantize());
    }
}

fn hash_page_into(hasher: &mut Sha256, page: &NotebookPage) {
    hasher.update(page.width.clamp(64, 8192).to_le_bytes());
    hasher.update(page.height.clamp(64, 8192).to_le_bytes());
    if let Ok(tpl) = serde_json::to_vec(&page.paper_template) {
        hash_len_bytes(hasher, &tpl);
    }
    if let Ok(col) = serde_json::to_vec(&page.paper_color) {
        hash_len_bytes(hasher, &col);
    }
    hasher.update((page.strokes.len() as u64).to_le_bytes());
    for stroke in &page.strokes {
        hash_stroke_into(hasher, stroke);
    }
    if let Some(backdrop) = page.pdf_backdrop {
        hasher.update([0xD0]);
        hasher.update((backdrop.pdf_index as u64).to_le_bytes());
        hasher.update((backdrop.page_index as u64).to_le_bytes());
    }
    let rot = page.backdrop_rotation % 4;
    if page.bookmarked
        || rot != 0
        || page.custom_image_backdrop.is_some()
        || page.custom_template_id.is_some()
    {
        hasher.update([0xD2]);
        hasher.update([u8::from(page.bookmarked), rot]);
        if let Some(img_idx) = page.custom_image_backdrop {
            hasher.update([1]);
            hasher.update((img_idx as u64).to_le_bytes());
        } else {
            hasher.update([0]);
        }
        if let Some(id) = &page.custom_template_id {
            hash_len_bytes(hasher, id.as_bytes());
        } else {
            hash_len_bytes(hasher, &[]);
        }
    }
    if !page.shapes.is_empty() || !page.texts.is_empty() {
        hasher.update([0xD5]);
        hasher.update((page.shapes.len() as u64).to_le_bytes());
        for shape in &page.shapes {
            if let Ok(shape_bytes) = serde_json::to_vec(&shape.sanitized()) {
                hash_len_bytes(hasher, &shape_bytes);
            }
        }
        hasher.update((page.texts.len() as u64).to_le_bytes());
        for text in &page.texts {
            if let Ok(text_bytes) = serde_json::to_vec(&text.sanitized()) {
                hash_len_bytes(hasher, &text_bytes);
            }
        }
    }
}

fn hash_document_body_into(hasher: &mut Sha256, doc: &Document) {
    hasher.update(doc.next_stroke_id.to_le_bytes());

    let pages = doc.resolved_pages();
    hasher.update((pages.len() as u64).to_le_bytes());
    for page in &pages {
        hash_page_into(hasher, page);
    }
    if !doc.pdf_documents.is_empty() {
        hasher.update([0xD1]);
        hasher.update((doc.pdf_documents.len() as u64).to_le_bytes());
        for pdf in &doc.pdf_documents {
            hash_len_bytes(hasher, pdf.name.as_bytes());
            hash_len_bytes(hasher, pdf.bytes.as_slice());
        }
    }
    if !doc.image_templates.is_empty() {
        hasher.update([0xD4]);
        hasher.update((doc.image_templates.len() as u64).to_le_bytes());
        for tpl in &doc.image_templates {
            hash_len_bytes(hasher, tpl.name.as_bytes());
            hash_len_bytes(hasher, tpl.png_bytes.as_slice());
        }
    }
}

fn hash_document_meta_into(hasher: &mut Sha256, doc: &Document) {
    let clean_title = drawcraft_doc::sanitize_notebook_title(&doc.title);
    hash_len_bytes(hasher, clean_title.as_bytes());
    if let Ok(cover_bytes) = serde_json::to_vec(&doc.cover) {
        hash_len_bytes(hasher, &cover_bytes);
    }
    let folder = drawcraft_doc::sanitize_folder_path(&doc.folder_path);
    let tags = drawcraft_doc::sanitize_notebook_tags(&doc.tags);
    hash_len_bytes(hasher, folder.as_bytes());
    hasher.update([u8::from(doc.favorite)]);
    hasher.update((tags.len() as u64).to_le_bytes());
    for tag in &tags {
        hash_len_bytes(hasher, tag.as_bytes());
    }
}

/// Computes a deterministic SHA-256 hash covering only page, ink, text, shape, image, and backdrop
/// content (excluding notebook metadata like title, cover, folder, favorite, or tags).
pub fn canonical_document_body_hash(doc: &Document) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"drawcraft-doc-body-v1");
    hash_document_body_into(&mut hasher, doc);
    bytes_to_hex(&hasher.finalize())
}

/// Computes a deterministic SHA-256 hash covering only notebook metadata (`title`, `cover`,
/// `folder_path`, `favorite`, and `tags`) for Last-Write-Wins metadata conflict resolution.
pub fn canonical_document_meta_hash(doc: &Document) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"drawcraft-doc-meta-v1");
    hash_document_meta_into(&mut hasher, doc);
    bytes_to_hex(&hasher.finalize())
}

/// Computes a deterministic SHA-256 content hash for a [`Document`] snapshot,
/// invariant across `.ncraft` v2 binary save/load and active-page serialization deduplication.
pub fn canonical_document_hash(doc: &Document) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"drawcraft-doc-canonical-v2");
    let clean_title = drawcraft_doc::sanitize_notebook_title(&doc.title);
    hash_len_bytes(&mut hasher, clean_title.as_bytes());
    if let Ok(cover_bytes) = serde_json::to_vec(&doc.cover) {
        hash_len_bytes(&mut hasher, &cover_bytes);
    }
    let active_idx = u64::try_from(doc.active_page_index()).unwrap_or(0);
    hasher.update(active_idx.to_le_bytes());
    hasher.update(doc.next_stroke_id.to_le_bytes());

    let pages = doc.resolved_pages();
    hasher.update((pages.len() as u64).to_le_bytes());
    for page in &pages {
        hash_page_into(&mut hasher, page);
    }
    if !doc.pdf_documents.is_empty() {
        hasher.update([0xD1]);
        hasher.update((doc.pdf_documents.len() as u64).to_le_bytes());
        for pdf in &doc.pdf_documents {
            hash_len_bytes(&mut hasher, pdf.name.as_bytes());
            hash_len_bytes(&mut hasher, pdf.bytes.as_slice());
        }
    }
    let folder = drawcraft_doc::sanitize_folder_path(&doc.folder_path);
    let tags = drawcraft_doc::sanitize_notebook_tags(&doc.tags);
    if !folder.is_empty() || doc.favorite || !tags.is_empty() {
        hasher.update([0xD3]);
        hash_len_bytes(&mut hasher, folder.as_bytes());
        hasher.update([u8::from(doc.favorite)]);
        hasher.update((tags.len() as u64).to_le_bytes());
        for tag in &tags {
            hash_len_bytes(&mut hasher, tag.as_bytes());
        }
    }
    if !doc.image_templates.is_empty() {
        hasher.update([0xD4]);
        hasher.update((doc.image_templates.len() as u64).to_le_bytes());
        for tpl in &doc.image_templates {
            hash_len_bytes(&mut hasher, tpl.name.as_bytes());
            hash_len_bytes(&mut hasher, tpl.png_bytes.as_slice());
        }
    }
    bytes_to_hex(&hasher.finalize())
}

/// Computes a deterministic SHA-256 content hash for [`UserStudioConfig`].
pub fn canonical_prefs_hash(prefs: &UserStudioConfig) -> String {
    let clean = prefs.sanitized();
    match serde_json::to_vec(&clean) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => String::new(),
    }
}

/// Returns current UNIX epoch time in milliseconds, never negative.
///
/// Uses `chrono` so the same code works natively and on `wasm32-unknown-unknown`
/// (where `std::time::SystemTime::now()` is unavailable).
pub fn now_unix_ms() -> i64 {
    chrono::Utc::now().timestamp_millis().max(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn normalize_server_url_supports_local_vpn_ipv6_and_rejects_hostile_urls() {
        assert_eq!(
            normalize_server_url("127.0.0.1:8787").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            normalize_server_url("  https://my-pc.tailnet.ts.net/  ").unwrap(),
            "https://my-pc.tailnet.ts.net"
        );
        assert_eq!(
            normalize_server_url("http://100.64.0.12:8787/").unwrap(),
            "http://100.64.0.12:8787"
        );
        assert_eq!(
            normalize_server_url("http://[::1]:8787").unwrap(),
            "http://[::1]:8787"
        );

        assert!(normalize_server_url("").is_err());
        assert!(normalize_server_url("ftp://127.0.0.1:8787").is_err());
        assert!(normalize_server_url("http://user:pass@127.0.0.1:8787").is_err());
        assert!(normalize_server_url("http://127.0.0.1:0").is_err());
        assert!(normalize_server_url("http://bad host:8787").is_err());
        assert!(normalize_server_url("http://127.0.0.1:8787?x=1").is_err());
    }

    #[test]
    fn username_password_and_notebook_id_validation_handle_edge_cases() {
        assert_eq!(
            normalize_username("  Alex_01 ").unwrap(),
            ("Alex_01".to_string(), "alex_01".to_string())
        );
        assert!(normalize_username("ab").is_err());
        assert!(normalize_username("_alex").is_err());
        assert!(normalize_username("alex@home").is_err());

        assert!(validate_password("correct-horse-123").is_ok());
        assert!(validate_password("short").is_err());
        assert!(validate_password("bad\npassword").is_err());

        assert_eq!(
            validate_notebook_id("nb-550e8400_1").unwrap(),
            "nb-550e8400_1"
        );
        assert!(validate_notebook_id("../etc").is_err());
        assert!(validate_notebook_id("").is_err());
    }

    #[test]
    fn canonical_hashes_are_deterministic_and_sensitive_to_edits() {
        let mut doc = Document::default();
        let h1 = canonical_document_hash(&doc);
        let h2 = canonical_document_hash(&doc);
        assert_eq!(h1.len(), 64);
        assert_eq!(h1, h2);

        doc.title = "Updated Title".to_string();
        let h3 = canonical_document_hash(&doc);
        assert_ne!(h1, h3);

        let mut prefs = UserStudioConfig::default();
        let p1 = canonical_prefs_hash(&prefs);
        prefs.theme = drawcraft_doc::UiTheme::Dark;
        let p2 = canonical_prefs_hash(&prefs);
        assert_ne!(p1, p2);
        prefs.set_folder_color(
            "School",
            Some(drawcraft_doc::NotebookCover::custom_rgb(0x25, 0x63, 0xEB).primary_color()),
        );
        let p3 = canonical_prefs_hash(&prefs);
        assert_ne!(p2, p3);
        prefs.set_tag_color(
            "exam",
            Some(drawcraft_doc::NotebookCover::custom_rgb(0x0D, 0x94, 0x88).primary_color()),
        );
        let p4 = canonical_prefs_hash(&prefs);
        assert_ne!(p3, p4);

        let mut org = Document::default();
        let base_hash = canonical_document_hash(&org);
        org.folder_path = "School/Math".to_string();
        let folder_hash = canonical_document_hash(&org);
        assert_ne!(base_hash, folder_hash);
        org.favorite = true;
        org.tags = vec!["exam".to_string()];
        let fav_tag_hash = canonical_document_hash(&org);
        assert_ne!(folder_hash, fav_tag_hash);
        org.bookmarked = true;
        org.sync_active_to_pages();
        assert_ne!(fav_tag_hash, canonical_document_hash(&org));

        // Body hash stays invariant across metadata edits; meta hash tracks metadata only.
        let mut a = Document::default();
        let b = Document {
            title: "Renamed".to_string(),
            folder_path: "Work/Meetings".to_string(),
            favorite: true,
            tags: vec!["urgent".to_string()],
            ..Default::default()
        };
        assert_eq!(
            canonical_document_body_hash(&a),
            canonical_document_body_hash(&b)
        );
        assert_ne!(
            canonical_document_meta_hash(&a),
            canonical_document_meta_hash(&b)
        );
        a.paper_template = drawcraft_doc::PaperTemplate::Grid;
        a.sync_active_to_pages();
        assert_ne!(
            canonical_document_body_hash(&a),
            canonical_document_body_hash(&b)
        );
    }

    #[test]
    fn email_normalization_and_validation() {
        assert_eq!(
            normalize_email("  Alex.Notes@Example.COM ").unwrap(),
            (
                "Alex.Notes@Example.COM".to_string(),
                "alex.notes@example.com".to_string()
            )
        );
        assert!(normalize_email("not-an-email").is_err());
        assert!(normalize_email("@example.com").is_err());
        assert!(normalize_email("user@localhost").is_err());
        assert!(normalize_email("user@.example.com").is_err());
        assert!(normalize_email("user@example..com").is_err());
        assert!(normalize_email("a b@example.com").is_err());
        assert_eq!(
            derive_username_from_email("alex.notes+1@example.com"),
            "alex.notes_1"
        );
    }

    #[test]
    fn discovery_and_force_overwrite_protocol_defaults() {
        let legacy_info = serde_json::json!({
            "status": "ok",
            "service": "drawcraft-server",
            "version": "0.1.0",
            "protocol_version": 1,
            "registration_open": true,
            "storage_engine": "redb",
            "max_notebook_bytes": 1024
        });
        let decoded: ServerInfoResponse = serde_json::from_value(legacy_info).unwrap();
        assert_eq!(decoded.local_discovery, LocalDiscoverySummary::default());

        let push = SyncPushRequest {
            client_cursor: 3,
            device_name: "laptop".to_string(),
            changes: Vec::new(),
            folders: Vec::new(),
            studio_prefs: None,
            force_overwrite: true,
        };
        let encoded = serde_json::to_string(&push).unwrap();
        let round: SyncPushRequest = serde_json::from_str(&encoded).unwrap();
        assert!(round.force_overwrite);

        let prune = PruneToLocalRequest {
            keep_notebook_ids: vec!["nb-1".to_string()],
            keep_folder_ids: vec!["fld-1".to_string()],
            device_name: "laptop".to_string(),
        };
        let prune_json = serde_json::to_string(&prune).unwrap();
        let prune_round: PruneToLocalRequest = serde_json::from_str(&prune_json).unwrap();
        assert_eq!(prune_round, prune);
    }
}
