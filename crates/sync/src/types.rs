use drawcraft_doc::Document;
use drawcraft_sync_protocol::{
    AcceptedFolderSync, AcceptedNotebookSync, DEFAULT_SERVER_URL, LocalFolderChange,
    LocalPrefsPush, ProtocolError, RemoteFolderRecord, RemotePrefsRecord, UserProfile, now_unix_ms,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const GLOBAL_SYNC_FILE: &str = "active_session.json";
pub const SYNC_MANIFEST_FILE: &str = "sync_manifest.json";
pub const ACCOUNTS_DIR: &str = "accounts";
pub const PROFILES_DIR: &str = "profiles";
pub const MAX_PROFILE_NICKNAME_LEN: usize = 48;
pub const SYNC_MANIFEST_SCHEMA_VERSION: u32 = 2;

/// Strict ownership boundary for workspaces, folders, notebooks, and sync manifests.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum OwnerId {
    LocalProfile(String),
    CloudUser(String),
}

impl OwnerId {
    pub fn as_key(&self) -> String {
        match self {
            Self::LocalProfile(id) => format!("local:{id}"),
            Self::CloudUser(id) => format!("cloud:{id}"),
        }
    }

    pub fn is_cloud(&self) -> bool {
        matches!(self, Self::CloudUser(_))
    }
}

/// Identifies which workspace the UI is currently viewing (`LocalProfile` vs `CloudUser`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id")]
pub enum ActiveWorkspace {
    LocalProfile(String),
    CloudUser(String),
}

/// Client-side local workspace profile (mandatory offline-first identity).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalProfile {
    pub local_profile_id: String,
    pub nickname: String,
    pub created_at_ms: i64,
    #[serde(default)]
    pub last_used_at_ms: i64,
    /// Cloud `user_id` that owns this local profile when it was created inside a signed-in
    /// account. Unlinked offline profiles (`None`) may be offered for migration to any account;
    /// profiles bound to one account are never offered to other accounts.
    #[serde(default)]
    pub owner_cloud_user_id: Option<String>,
}

impl LocalProfile {
    /// `true` when this profile is unlinked or owned by `cloud_user_id`.
    #[must_use]
    pub fn is_accessible_to_cloud_user(&self, cloud_user_id: Option<&str>) -> bool {
        match (&self.owner_cloud_user_id, cloud_user_id) {
            (None, _) => true,
            (Some(owner), Some(current)) => owner == current,
            (Some(_), None) => false,
        }
    }
}

/// Explicit per-entity synchronization state tracked locally and surfaced in UI badges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntitySyncStatus {
    #[default]
    LocalOnly,
    PendingPush,
    Synced,
    Conflict,
    SyncError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveAccountSession {
    pub server_url: String,
    pub user: UserProfile,
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at_ms: i64,
    pub refresh_expires_at_ms: i64,
    pub device_name: String,
    pub last_auth_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSyncState {
    pub server_url: String,
    pub device_name: String,
    #[serde(default)]
    pub session: Option<ActiveAccountSession>,
    #[serde(default)]
    pub local_profiles: Vec<LocalProfile>,
    #[serde(default)]
    pub active_local_profile_id: String,
    #[serde(default)]
    pub active_workspace: Option<ActiveWorkspace>,
    #[serde(default)]
    pub needs_initial_nickname: bool,
    #[serde(default)]
    pub client_id: String,
}

impl GlobalSyncState {
    pub fn with_defaults(device_name: String) -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            device_name,
            session: None,
            local_profiles: Vec::new(),
            active_local_profile_id: String::new(),
            active_workspace: None,
            needs_initial_nickname: false,
            client_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// The signed-in cloud account, if any.
    #[must_use]
    pub fn active_cloud_user_id(&self) -> Option<&str> {
        self.session.as_ref().map(|s| s.user.user_id.as_str())
    }

    /// Local profiles the current session may see (unlinked, or owned by the signed-in account).
    #[must_use]
    pub fn accessible_local_profiles(&self) -> Vec<&LocalProfile> {
        let uid = self.active_cloud_user_id();
        self.local_profiles
            .iter()
            .filter(|p| p.is_accessible_to_cloud_user(uid))
            .collect()
    }

    pub fn active_local_profile(&self) -> Option<&LocalProfile> {
        let uid = self.active_cloud_user_id();
        self.local_profiles
            .iter()
            .find(|p| {
                p.local_profile_id == self.active_local_profile_id
                    && p.is_accessible_to_cloud_user(uid)
            })
            .or_else(|| {
                self.local_profiles
                    .iter()
                    .find(|p| p.is_accessible_to_cloud_user(uid))
            })
    }

    pub fn is_viewing_cloud(&self) -> bool {
        match (&self.session, &self.active_workspace) {
            (Some(sess), Some(ActiveWorkspace::CloudUser(uid))) => sess.user.user_id == *uid,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotebookSyncMeta {
    pub notebook_id: String,
    #[serde(default)]
    pub owner_id: Option<OwnerId>,
    #[serde(default)]
    pub status: EntitySyncStatus,
    #[serde(default)]
    pub local_file: Option<String>,
    pub server_version: u64,
    pub last_synced_hash: String,
    #[serde(default)]
    pub last_synced_body_hash: String,
    #[serde(default)]
    pub last_synced_meta_hash: String,
    pub cloud_dirty: bool,
    pub last_synced_at_ms: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default)]
    pub deleted_at_ms: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub conflict_of_notebook_id: Option<String>,
}

impl NotebookSyncMeta {
    pub fn effective_status(&self, is_cloud: bool) -> EntitySyncStatus {
        if !is_cloud {
            return EntitySyncStatus::LocalOnly;
        }
        if self.status == EntitySyncStatus::Conflict {
            return EntitySyncStatus::Conflict;
        }
        if self.status == EntitySyncStatus::SyncError && self.last_error.is_some() {
            return EntitySyncStatus::SyncError;
        }
        if self.cloud_dirty
            || self.server_version == 0
            || self.status == EntitySyncStatus::PendingPush
        {
            return EntitySyncStatus::PendingPush;
        }
        EntitySyncStatus::Synced
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderSyncMeta {
    pub folder_id: String,
    #[serde(default)]
    pub owner_id: Option<OwnerId>,
    pub path: String,
    #[serde(default)]
    pub color_hex: Option<String>,
    #[serde(default)]
    pub status: EntitySyncStatus,
    pub server_version: u64,
    pub cloud_dirty: bool,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub deleted_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotebookTombstone {
    pub notebook_id: String,
    pub base_version: u64,
    pub deleted_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncManifest {
    pub schema_version: u32,
    #[serde(default)]
    pub owner_id: Option<OwnerId>,
    pub last_server_cursor: u64,
    pub prefs_version: u64,
    #[serde(default)]
    pub prefs_hash: String,
    #[serde(default)]
    pub prefs_dirty: bool,
    #[serde(default)]
    pub offline_edits_pending: bool,
    #[serde(default)]
    pub notebooks: Vec<NotebookSyncMeta>,
    #[serde(default)]
    pub folders: Vec<FolderSyncMeta>,
    #[serde(default)]
    pub tombstones: Vec<NotebookTombstone>,
    #[serde(default)]
    pub last_sync_at_ms: Option<i64>,
}

impl Default for SyncManifest {
    fn default() -> Self {
        Self {
            schema_version: SYNC_MANIFEST_SCHEMA_VERSION,
            owner_id: None,
            last_server_cursor: 0,
            prefs_version: 0,
            prefs_hash: String::new(),
            prefs_dirty: false,
            offline_edits_pending: false,
            notebooks: Vec::new(),
            folders: Vec::new(),
            tombstones: Vec::new(),
            last_sync_at_ms: None,
        }
    }
}

impl SyncManifest {
    pub fn meta(&self, notebook_id: &str) -> Option<&NotebookSyncMeta> {
        self.notebooks
            .iter()
            .find(|item| item.notebook_id == notebook_id)
    }

    pub fn id_for_local_file(&self, file_name: &str) -> Option<&str> {
        self.notebooks
            .iter()
            .find(|item| item.local_file.as_deref() == Some(file_name))
            .map(|item| item.notebook_id.as_str())
    }

    pub fn bind_local_file(&mut self, notebook_id: &str, file_name: &str) {
        if let Some(existing) = self
            .notebooks
            .iter_mut()
            .find(|item| item.notebook_id == notebook_id)
        {
            existing.local_file = Some(file_name.to_string());
        }
    }

    pub fn ensure_notebook(&mut self, notebook_id: &str, current_hash: &str) {
        self.mark_notebook_dirty(notebook_id, current_hash);
    }

    pub fn mark_notebook_dirty(&mut self, notebook_id: &str, current_hash: &str) {
        let is_cloud = self.owner_id.as_ref().is_some_and(OwnerId::is_cloud);
        let now = now_unix_ms();
        if let Some(existing) = self
            .notebooks
            .iter_mut()
            .find(|item| item.notebook_id == notebook_id)
        {
            let dirty = existing.server_version == 0 || existing.last_synced_hash != current_hash;
            existing.cloud_dirty = dirty;
            existing.updated_at_ms = existing.updated_at_ms.max(now);
            existing.last_error = None;
            if existing.conflict_of_notebook_id.is_some() && !is_cloud {
                existing.status = EntitySyncStatus::LocalOnly;
            } else if existing.conflict_of_notebook_id.is_some() && existing.server_version == 0 {
                existing.status = EntitySyncStatus::Conflict;
            } else if !is_cloud {
                existing.status = EntitySyncStatus::LocalOnly;
            } else if dirty {
                existing.status = EntitySyncStatus::PendingPush;
            } else {
                existing.status = EntitySyncStatus::Synced;
            }
        } else {
            self.notebooks.push(NotebookSyncMeta {
                notebook_id: notebook_id.to_string(),
                owner_id: self.owner_id.clone(),
                status: if is_cloud {
                    EntitySyncStatus::PendingPush
                } else {
                    EntitySyncStatus::LocalOnly
                },
                local_file: None,
                server_version: 0,
                last_synced_hash: String::new(),
                last_synced_body_hash: String::new(),
                last_synced_meta_hash: String::new(),
                cloud_dirty: true,
                last_synced_at_ms: 0,
                updated_at_ms: now,
                deleted_at_ms: None,
                last_error: None,
                conflict_of_notebook_id: None,
            });
        }
    }

    pub fn mark_notebook_conflict(
        &mut self,
        notebook_id: &str,
        conflict_of_notebook_id: &str,
        current_hash: &str,
    ) {
        self.mark_notebook_dirty(notebook_id, current_hash);
        if let Some(existing) = self
            .notebooks
            .iter_mut()
            .find(|item| item.notebook_id == notebook_id)
        {
            existing.conflict_of_notebook_id = Some(conflict_of_notebook_id.to_string());
            existing.status = if self.owner_id.as_ref().is_some_and(OwnerId::is_cloud) {
                EntitySyncStatus::Conflict
            } else {
                EntitySyncStatus::LocalOnly
            };
        }
    }

    pub fn record_local_delete(&mut self, notebook_id: &str, deleted_at_ms: i64) {
        let existing_version = self
            .notebooks
            .iter()
            .find(|item| item.notebook_id == notebook_id)
            .map_or(0, |item| item.server_version);
        self.notebooks
            .retain(|item| item.notebook_id != notebook_id);

        // Only previously synced notebooks require a remote tombstone.
        if existing_version == 0 {
            return;
        }
        if let Some(tombstone) = self
            .tombstones
            .iter_mut()
            .find(|item| item.notebook_id == notebook_id)
        {
            tombstone.base_version = tombstone.base_version.max(existing_version);
            tombstone.deleted_at_ms = deleted_at_ms;
        } else {
            self.tombstones.push(NotebookTombstone {
                notebook_id: notebook_id.to_string(),
                base_version: existing_version,
                deleted_at_ms,
            });
        }
    }

    pub fn mark_prefs_dirty(&mut self, current_hash: &str) {
        self.prefs_dirty = self.prefs_version == 0 || self.prefs_hash != current_hash;
    }

    pub fn mark_notebook_synced(
        &mut self,
        notebook_id: &str,
        new_version: u64,
        synced_hash: String,
        current_hash: &str,
        synced_at_ms: i64,
    ) {
        let still_dirty = current_hash != synced_hash;
        let is_cloud = self.owner_id.as_ref().is_some_and(OwnerId::is_cloud);
        let next_status = if !is_cloud {
            EntitySyncStatus::LocalOnly
        } else if still_dirty {
            EntitySyncStatus::PendingPush
        } else {
            EntitySyncStatus::Synced
        };
        if let Some(meta) = self
            .notebooks
            .iter_mut()
            .find(|item| item.notebook_id == notebook_id)
        {
            meta.owner_id = self.owner_id.clone();
            meta.server_version = new_version;
            meta.last_synced_hash = synced_hash;
            meta.cloud_dirty = still_dirty;
            meta.last_synced_at_ms = synced_at_ms;
            meta.deleted_at_ms = None;
            meta.last_error = None;
            if !still_dirty {
                meta.conflict_of_notebook_id = None;
            }
            meta.status = next_status;
        } else {
            self.notebooks.push(NotebookSyncMeta {
                notebook_id: notebook_id.to_string(),
                owner_id: self.owner_id.clone(),
                status: next_status,
                local_file: None,
                server_version: new_version,
                last_synced_hash: synced_hash,
                last_synced_body_hash: String::new(),
                last_synced_meta_hash: String::new(),
                cloud_dirty: still_dirty,
                last_synced_at_ms: synced_at_ms,
                updated_at_ms: synced_at_ms,
                deleted_at_ms: None,
                last_error: None,
                conflict_of_notebook_id: None,
            });
        }
    }

    pub fn reassign_owner(&mut self, next_owner: OwnerId, mark_pending: bool) {
        let is_cloud = next_owner.is_cloud();
        self.owner_id = Some(next_owner.clone());
        for nb in &mut self.notebooks {
            nb.owner_id = Some(next_owner.clone());
            if mark_pending && is_cloud {
                nb.server_version = 0;
                nb.last_synced_hash.clear();
                nb.last_synced_body_hash.clear();
                nb.last_synced_meta_hash.clear();
                nb.cloud_dirty = true;
                nb.last_synced_at_ms = 0;
                nb.status = EntitySyncStatus::PendingPush;
                nb.last_error = None;
            } else if !is_cloud {
                nb.status = EntitySyncStatus::LocalOnly;
            }
        }
        for folder in &mut self.folders {
            folder.owner_id = Some(next_owner.clone());
            if mark_pending && is_cloud {
                folder.server_version = 0;
                folder.cloud_dirty = true;
                folder.status = EntitySyncStatus::PendingPush;
            } else if !is_cloud {
                folder.status = EntitySyncStatus::LocalOnly;
            }
        }
        if mark_pending && is_cloud {
            self.last_server_cursor = 0;
            self.prefs_version = 0;
            self.prefs_hash.clear();
            self.prefs_dirty = true;
        }
    }

    pub fn clear_tombstone(&mut self, notebook_id: &str) {
        self.tombstones
            .retain(|item| item.notebook_id != notebook_id);
    }

    pub fn pending_changes_count(&self) -> usize {
        let nb_dirty = self
            .notebooks
            .iter()
            .filter(|m| m.cloud_dirty || m.server_version == 0)
            .count();
        let folder_dirty = self
            .folders
            .iter()
            .filter(|f| f.cloud_dirty || f.server_version == 0 || f.deleted_at_ms.is_some())
            .count();
        nb_dirty + folder_dirty + self.tombstones.len() + usize::from(self.prefs_dirty)
    }

    pub fn reset_for_fresh_sync(&mut self) {
        self.last_server_cursor = 0;
        self.last_sync_at_ms = Some(now_unix_ms());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SyncConnectionState {
    #[default]
    SignedOut,
    Offline,
    Syncing,
    Synced,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncStatusSummary {
    pub signed_in: bool,
    pub viewing_cloud: bool,
    pub server_url: String,
    pub device_name: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub active_local_profile_id: Option<String>,
    pub active_local_profile_name: Option<String>,
    pub connection: SyncConnectionState,
    pub pending_changes: usize,
    pub last_sync_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub last_conflict_notice: Option<String>,
}

impl SyncStatusSummary {
    pub fn workspace_badge_text(&self) -> String {
        if self.viewing_cloud {
            self.email
                .clone()
                .or_else(|| self.username.as_ref().map(|u| format!("@{u}")))
                .unwrap_or_else(|| "Cloud".to_string())
        } else {
            self.active_local_profile_name
                .clone()
                .unwrap_or_else(|| "Personal".to_string())
        }
    }
}

#[derive(Clone, Debug)]
pub struct LocalNotebookSnapshot {
    pub notebook_id: String,
    pub base_version: u64,
    pub cloud_dirty: bool,
    pub content_hash: String,
    pub doc: Document,
}

#[derive(Clone, Debug)]
pub struct SyncBatch {
    pub client_cursor: u64,
    pub device_name: String,
    pub local_docs: Vec<LocalNotebookSnapshot>,
    pub tombstones: Vec<NotebookTombstone>,
    pub folder_changes: Vec<LocalFolderChange>,
    pub prefs_push: Option<LocalPrefsPush>,
    pub force_overwrite: bool,
}

/// Summary presented when offline/divergent edits require choosing Server vs Local version.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConflictPreview {
    pub account_name: String,
    pub account_email: String,
    pub server_notebook_count: usize,
    pub local_notebook_count: usize,
    pub server_cursor: u64,
    pub local_pending_changes: usize,
    pub local_dirty_titles: Vec<String>,
    pub server_notebook_titles: Vec<String>,
    pub offline_edits_detected: bool,
}

#[derive(Clone, Debug)]
pub struct RemoteNotebookUpdate {
    pub notebook_id: String,
    pub version: u64,
    pub cursor: u64,
    pub deleted: bool,
    pub updated_at_ms: i64,
    pub deleted_at_ms: Option<i64>,
    pub content_hash: String,
    pub body_hash: String,
    pub meta_hash: String,
    pub meta_updated_at_ms: i64,
    pub document: Option<Document>,
}

#[derive(Clone, Debug)]
pub struct ConflictResolutionCopy {
    pub original_notebook_id: String,
    pub conflict_notebook_id: String,
    pub conflict_title: String,
    pub local_doc: Document,
    pub local_hash: String,
}

#[derive(Clone, Debug)]
pub struct SyncCycleOutcome {
    pub new_cursor: u64,
    pub refreshed_session: Option<ActiveAccountSession>,
    pub server_user: UserProfile,
    pub accepted_uploads: Vec<AcceptedNotebookSync>,
    pub accepted_folders: Vec<AcceptedFolderSync>,
    pub cleared_tombstones: Vec<String>,
    pub resolved_conflicts: Vec<ConflictResolutionCopy>,
    pub downloaded_updates: Vec<RemoteNotebookUpdate>,
    pub downloaded_folders: Vec<RemoteFolderRecord>,
    pub updated_prefs: Option<RemotePrefsRecord>,
    pub ack_prefs: Option<(u64, String)>,
    pub prefs_conflict: Option<RemotePrefsRecord>,
    pub pushed_count: usize,
    pub pulled_count: usize,
    pub conflict_count: usize,
    pub synced_at_ms: i64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SyncError {
    #[error("offline or unreachable server: {0}")]
    Network(String),
    #[error("session expired or unauthorized: {0}")]
    Unauthorized(String),
    #[error("server error ({status} {code}): {message}")]
    Server {
        status: u16,
        code: String,
        message: String,
    },
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
    #[error("local storage error: {0}")]
    Io(String),
    #[error("invalid server response: {0}")]
    InvalidResponse(String),
}
