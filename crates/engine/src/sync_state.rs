use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use drawcraft_color::Color;
use drawcraft_doc::{
    Document, UserStudioConfig, sanitize_folder_path, sanitize_library_folders,
    sanitize_notebook_id, sanitize_notebook_title,
};
use drawcraft_sync::{
    ActiveAccountSession, ActiveWorkspace, EntitySyncStatus, FolderSyncMeta, GLOBAL_SYNC_FILE,
    GlobalSyncState, LocalNotebookSnapshot, LocalProfile, OwnerId, RemoteNotebookUpdate, SyncBatch,
    SyncConnectionState, SyncCycleOutcome, SyncManifest, SyncStatusSummary, account_library_dir,
    default_device_name, load_global_state, load_manifest, profile_library_dir,
    sanitize_account_dir_segment, sanitize_client_id, sanitize_profile_nickname, save_global_state,
    save_manifest,
};
use drawcraft_sync_protocol::{
    DEFAULT_SERVER_URL, LocalDiscoveryReport, LocalFolderChange, LocalPrefsPush,
    RemoteFolderRecord, canonical_document_body_hash, canonical_document_hash,
    canonical_prefs_hash, normalize_server_url, now_unix_ms, sanitize_device_name,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::library::{
    LIBRARY_MARKER_FILE, LibraryInitMode, STUDIO_PREFS_FILE, TrashedNotebook,
    count_ncraft_files_in_dir, write_document_snapshot,
};
use crate::{MAX_NOTEBOOKS, Session};

const WEB_CACHE_VERSION: u32 = 2;
const LEGACY_WEB_CACHE_VERSION: u32 = 1;
const TRASH_SUBDIR: &str = "trash";

/// Controls how switching into an authenticated cloud account treats local profiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthWorkspaceMode {
    /// Switch workspace to the cloud account without reading, copying, or altering local profiles.
    SwitchOnly,
    /// Promote an active local profile on first cloud registration (preserves UUIDs, marks `PendingPush`).
    PromoteLocalProfile { source_profile_id: String },
    /// Merge an active local profile into an existing cloud account (preserves UUIDs, zero duplication).
    MergeLocalProfile { source_profile_id: String },
}

/// Summary of a notebook inside a local profile (for manual cloud transfer pickers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalNotebookEntry {
    pub notebook_id: String,
    pub title: String,
    pub page_count: usize,
    pub folder_path: String,
}

/// Summary returned after migrating and deleting a local account into a synced cloud account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalMigrationSummary {
    pub source_profile_id: String,
    pub source_nickname: String,
    pub target_account_name: String,
    pub moved_notebooks: usize,
    pub moved_trash: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CachedTrashEntry {
    pub(crate) doc: Document,
    pub(crate) deleted_at_ms: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct CachedLibraryBundle {
    pub(crate) notebooks: Vec<Document>,
    #[serde(default)]
    pub(crate) active_notebook: usize,
    #[serde(default)]
    pub(crate) trash: Vec<CachedTrashEntry>,
    #[serde(default)]
    pub(crate) prefs: UserStudioConfig,
    #[serde(default)]
    pub(crate) manifest: SyncManifest,
    #[serde(default)]
    pub(crate) slot_ids: Vec<String>,
    #[serde(default)]
    pub(crate) slot_dirty_hash: Vec<bool>,
    #[serde(default)]
    pub(crate) slot_placeholder_blank: Vec<bool>,
    #[serde(default)]
    pub(crate) prefs_dirty_pending: bool,
    #[serde(default)]
    pub(crate) last_dirty_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WebWorkspaceCache {
    version: u32,
    global: GlobalSyncState,
    #[serde(default)]
    guest: Option<CachedLibraryBundle>,
    #[serde(default)]
    local_profiles: HashMap<String, CachedLibraryBundle>,
    #[serde(default)]
    accounts: HashMap<String, CachedLibraryBundle>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionSyncState {
    pub(crate) desktop_root: Option<PathBuf>,
    pub(crate) track_manifest: bool,
    pub(crate) web_cache_enabled: bool,
    pub(crate) web_local_bundles: HashMap<String, CachedLibraryBundle>,
    pub(crate) web_account_bundles: HashMap<String, CachedLibraryBundle>,
    pub(crate) global: GlobalSyncState,
    pub(crate) manifest: SyncManifest,
    pub(crate) slot_ids: Vec<String>,
    pub(crate) slot_dirty_hash: Vec<bool>,
    pub(crate) slot_placeholder_blank: Vec<bool>,
    pub(crate) prefs_dirty_pending: bool,
    pub(crate) connection: SyncConnectionState,
    pub(crate) last_error: Option<String>,
    pub(crate) last_conflict_notice: Option<String>,
    pub(crate) last_dirty_at_ms: Option<i64>,
    pub(crate) sync_in_flight: bool,
    pub(crate) network_probed_offline: bool,
}

impl Default for SessionSyncState {
    fn default() -> Self {
        Self {
            desktop_root: None,
            track_manifest: false,
            web_cache_enabled: false,
            web_local_bundles: HashMap::new(),
            web_account_bundles: HashMap::new(),
            global: GlobalSyncState::with_defaults(default_device_name()),
            manifest: SyncManifest::default(),
            slot_ids: vec![String::new()],
            slot_dirty_hash: vec![false],
            slot_placeholder_blank: vec![false],
            prefs_dirty_pending: false,
            connection: SyncConnectionState::SignedOut,
            last_error: None,
            last_conflict_notice: None,
            last_dirty_at_ms: None,
            sync_in_flight: false,
            network_probed_offline: false,
        }
    }
}

impl Session {
    /// Returns `true` when workspace persistence (desktop directory or web browser cache) is active.
    pub fn is_sync_persistence_enabled(&self) -> bool {
        self.sync.desktop_root.is_some() || self.sync.web_cache_enabled
    }

    /// Opens (or initializes) a browser-cached library with per-profile & per-account isolation.
    pub fn open_web_cache(raw_json: Option<&str>) -> Self {
        if let Some(mut cache) = raw_json
            .and_then(|raw| serde_json::from_str::<WebWorkspaceCache>(raw).ok())
            .filter(|c| matches!(c.version, LEGACY_WEB_CACHE_VERSION | WEB_CACHE_VERSION))
        {
            sanitize_global_state(&mut cache.global);
            cache.global.needs_initial_nickname = false;
            migrate_web_cache_if_needed(&mut cache);
            let viewing_cloud = cache.global.is_viewing_cloud();
            let (bundle, owner, connection) =
                if viewing_cloud && let Some(ref acct) = cache.global.session {
                    let b = cache
                        .accounts
                        .get(&acct.user.user_id)
                        .cloned()
                        .unwrap_or_else(placeholder_web_bundle);
                    (
                        b,
                        OwnerId::CloudUser(acct.user.user_id.clone()),
                        SyncConnectionState::Offline,
                    )
                } else {
                    let profile_id = ensure_default_profile_in_global(&mut cache.global);
                    let b = cache
                        .local_profiles
                        .get(&profile_id)
                        .cloned()
                        .unwrap_or_else(starter_web_bundle);
                    let conn = if cache.global.session.is_some() {
                        SyncConnectionState::Offline
                    } else {
                        SyncConnectionState::SignedOut
                    };
                    (b, OwnerId::LocalProfile(profile_id), conn)
                };

            let mut session = restore_web_bundle(&bundle);
            session.sync.global = cache.global;
            session.sync.web_cache_enabled = true;
            session.sync.track_manifest = true;
            session.sync.web_local_bundles = cache.local_profiles;
            session.sync.web_account_bundles = cache.accounts;
            session.sync.connection = connection;
            session.sync.manifest.owner_id = Some(owner);
            session.sync_reconcile_slots();
            return session;
        }

        let mut session = Self::default();
        // Fresh web workspaces start completely empty (zero fake/example notebooks) and ready to create.
        session.replace_notebooks_for_library(Vec::new(), None);
        session.sync.global = GlobalSyncState::with_defaults("Web Browser".to_string());
        session.sync.global.needs_initial_nickname = false;
        let pid = ensure_default_profile_in_global(&mut session.sync.global);
        session.sync.global.active_workspace = Some(ActiveWorkspace::LocalProfile(pid.clone()));
        session.sync.web_cache_enabled = true;
        session.sync.track_manifest = true;
        session.sync.connection = SyncConnectionState::SignedOut;
        session.sync.manifest.owner_id = Some(OwnerId::LocalProfile(pid));
        session.sync_reconcile_slots();
        session
    }

    /// Snapshots the active workspace, trash, studio preferences, and isolated profile/account caches.
    pub fn export_web_cache_json(&mut self) -> Result<String, String> {
        self.sync_active_notebook_slot();
        self.save_current_web_bundle();
        let cache = WebWorkspaceCache {
            version: WEB_CACHE_VERSION,
            global: self.sync.global.clone(),
            guest: None,
            local_profiles: self.sync.web_local_bundles.clone(),
            accounts: self.sync.web_account_bundles.clone(),
        };
        serde_json::to_string(&cache)
            .map_err(|err| format!("failed to serialize web workspace cache: {err}"))
    }

    pub(crate) fn save_current_web_bundle(&mut self) {
        if !self.sync.web_cache_enabled {
            return;
        }
        let current = self.capture_web_bundle();
        if self.sync.global.is_viewing_cloud()
            && let Some(ref acct) = self.sync.global.session
        {
            self.sync
                .web_account_bundles
                .insert(acct.user.user_id.clone(), current);
        } else {
            let pid = ensure_default_profile_in_global(&mut self.sync.global);
            self.sync.web_local_bundles.insert(pid, current);
        }
    }

    fn capture_web_bundle(&self) -> CachedLibraryBundle {
        let notebooks = self
            .notebooks
            .iter()
            .map(|doc| {
                let mut d = (**doc).clone();
                d.sync_active_to_pages();
                d
            })
            .collect();
        let trash = self
            .trashed_notebooks
            .iter()
            .map(|entry| CachedTrashEntry {
                doc: (*entry.doc).clone(),
                deleted_at_ms: entry.deleted_at_ms,
            })
            .collect();
        CachedLibraryBundle {
            notebooks,
            active_notebook: self.active_notebook,
            trash,
            prefs: self.prefs.clone(),
            manifest: self.sync.manifest.clone(),
            slot_ids: self.sync.slot_ids.clone(),
            slot_dirty_hash: self.sync.slot_dirty_hash.clone(),
            slot_placeholder_blank: self.sync.slot_placeholder_blank.clone(),
            prefs_dirty_pending: self.sync.prefs_dirty_pending,
            last_dirty_at_ms: self.sync.last_dirty_at_ms,
        }
    }

    /// Opens the desktop library root with mandatory local profiles and strict `owner_id` isolation.
    pub fn open_desktop_library(root_dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&root_dir);
        let global_path = root_dir.join(GLOBAL_SYNC_FILE);
        let first_access = !global_path.exists() && !has_legacy_root_library(&root_dir);
        let mut global = load_global_state(&root_dir);
        let mut save_global = first_access;
        if first_access {
            global.needs_initial_nickname = true;
        }
        if global.client_id.is_empty() {
            global.client_id = Uuid::new_v4().to_string();
            save_global = true;
        }
        if ensure_desktop_default_profile(&root_dir, &mut global) {
            save_global = true;
        }
        if save_global {
            let _ = save_global_state(&root_dir, &global);
        }

        let viewing_cloud = global.is_viewing_cloud();
        let (mut session, owner, is_placeholder, connection) =
            if viewing_cloud && let Some(ref acct) = global.session {
                let acct_dir = account_library_dir(&root_dir, &acct.user.user_id);
                let (s, placeholder) =
                    Self::open_library_mode(acct_dir, LibraryInitMode::BlankPlaceholderOnly);
                (
                    s,
                    OwnerId::CloudUser(acct.user.user_id.clone()),
                    placeholder,
                    SyncConnectionState::Offline,
                )
            } else {
                let pid = ensure_default_profile_in_global(&mut global);
                let prof_dir = profile_library_dir(&root_dir, &pid);
                let (s, placeholder) =
                    Self::open_library_mode(prof_dir, LibraryInitMode::SeedStartersIfFresh);
                let conn = if global.session.is_some() {
                    SyncConnectionState::Offline
                } else {
                    SyncConnectionState::SignedOut
                };
                (s, OwnerId::LocalProfile(pid), placeholder, conn)
            };

        session.sync.desktop_root = Some(root_dir);
        session.sync.track_manifest = true;
        session.sync.global = global;
        session.sync.connection = connection;
        session.sync_init_from_disk(is_placeholder, owner);
        session
    }

    pub fn is_signed_in(&self) -> bool {
        self.sync.global.session.is_some()
    }

    pub fn is_viewing_cloud(&self) -> bool {
        self.sync.global.is_viewing_cloud()
    }

    pub fn local_profiles(&self) -> &[LocalProfile] {
        &self.sync.global.local_profiles
    }

    /// Local profiles the active session may see: unlinked ones, or those owned by the currently
    /// signed-in cloud user. Profiles linked to a different cloud account are excluded.
    pub fn accessible_local_profiles(&self) -> Vec<LocalProfile> {
        self.sync
            .global
            .accessible_local_profiles()
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn active_local_profile(&self) -> Option<&LocalProfile> {
        self.sync.global.active_local_profile()
    }

    pub fn desktop_root_dir(&self) -> Option<&Path> {
        self.sync.desktop_root.as_deref()
    }

    pub fn active_account_session(&self) -> Option<&ActiveAccountSession> {
        self.sync.global.session.as_ref()
    }

    pub fn server_url(&self) -> &str {
        self.sync
            .global
            .session
            .as_ref()
            .map_or(self.sync.global.server_url.as_str(), |s| {
                s.server_url.as_str()
            })
    }

    pub fn device_name(&self) -> &str {
        &self.sync.global.device_name
    }

    pub fn client_id(&self) -> &str {
        &self.sync.global.client_id
    }

    pub fn needs_initial_nickname(&self) -> bool {
        self.sync.global.needs_initial_nickname
    }

    pub fn offline_edits_pending(&self) -> bool {
        self.sync.manifest.offline_edits_pending
    }

    pub fn set_network_offline_state(&mut self, offline: bool) {
        self.sync.network_probed_offline = offline;
    }

    /// Returns ONLY aggregate counts of unsynced local accounts and their notebooks (no PII).
    pub fn local_discovery_report(&self) -> LocalDiscoveryReport {
        if self.sync.global.needs_initial_nickname {
            return LocalDiscoveryReport {
                client_id: self.sync.global.client_id.clone(),
                unsynced_local_accounts: 0,
                total_local_notebooks: 0,
            };
        }
        let accessible = self.sync.global.accessible_local_profiles();
        let unsynced_local_accounts = accessible.len();
        let total_local_notebooks = accessible
            .iter()
            .map(|p| self.count_local_profile_notebooks(&p.local_profile_id))
            .sum();
        LocalDiscoveryReport {
            client_id: self.sync.global.client_id.clone(),
            unsynced_local_accounts,
            total_local_notebooks,
        }
    }

    /// Completes the first-launch local user nickname prompt and starts the local session.
    pub fn complete_first_access_nickname(
        &mut self,
        nickname_raw: &str,
    ) -> Result<LocalProfile, String> {
        if nickname_raw.trim().is_empty() {
            return Err("Please enter a nickname to start your session.".to_string());
        }
        let clean_nick = sanitize_profile_nickname(nickname_raw);
        let target_id = ensure_default_profile_in_global(&mut self.sync.global);
        let now = now_unix_ms();
        let mut updated_profile = None;
        for prof in &mut self.sync.global.local_profiles {
            if prof.local_profile_id == target_id {
                prof.nickname = clean_nick.clone();
                prof.last_used_at_ms = now;
                updated_profile = Some(prof.clone());
                break;
            }
        }
        self.sync.global.needs_initial_nickname = false;
        self.sync_persist_global_state();
        self.save_current_web_bundle();
        self.bump_revision();
        updated_profile.ok_or_else(|| "failed to initialize local profile".to_string())
    }

    /// Returns the explicit synchronization state for notebook slot `index`.
    pub fn notebook_sync_status(&self, index: usize) -> EntitySyncStatus {
        if !self.is_viewing_cloud() {
            return EntitySyncStatus::LocalOnly;
        }
        if self
            .sync
            .slot_placeholder_blank
            .get(index)
            .copied()
            .unwrap_or(false)
        {
            return EntitySyncStatus::LocalOnly;
        }
        if self
            .sync
            .slot_dirty_hash
            .get(index)
            .copied()
            .unwrap_or(false)
        {
            return EntitySyncStatus::PendingPush;
        }
        let Some(id) = self.sync.slot_ids.get(index) else {
            return EntitySyncStatus::PendingPush;
        };
        match self.sync.manifest.meta(id) {
            Some(meta) => meta.effective_status(true),
            None => EntitySyncStatus::PendingPush,
        }
    }

    /// Counts pending un-uploaded edits for the connected cloud account, even while viewing a local profile.
    pub fn cloud_pending_changes_count(&self) -> usize {
        let Some(ref acct) = self.sync.global.session else {
            return 0;
        };
        if self.is_viewing_cloud() {
            return self.compute_pending_changes_fast();
        }
        if let Some(ref root) = self.sync.desktop_root {
            let manifest = load_manifest(&account_library_dir(root, &acct.user.user_id));
            return manifest.pending_changes_count();
        }
        if self.sync.web_cache_enabled
            && let Some(bundle) = self.sync.web_account_bundles.get(&acct.user.user_id)
        {
            return bundle.manifest.pending_changes_count();
        }
        0
    }

    pub fn sync_status_summary(&self) -> SyncStatusSummary {
        let viewing_cloud = self.is_viewing_cloud();
        let pending_changes = if viewing_cloud {
            self.compute_pending_changes_fast()
        } else {
            self.cloud_pending_changes_count()
        };
        let (username, email, display_name) = match self.sync.global.session.as_ref() {
            Some(acct) => (
                Some(acct.user.username.clone()),
                acct.user.email.clone(),
                Some(acct.user.display_name.clone()),
            ),
            None => (None, None, None),
        };
        let active_prof = self.sync.global.active_local_profile();
        SyncStatusSummary {
            signed_in: self.sync.global.session.is_some(),
            viewing_cloud,
            server_url: self.server_url().to_string(),
            device_name: self.sync.global.device_name.clone(),
            username,
            email,
            display_name,
            active_local_profile_id: active_prof.map(|p| p.local_profile_id.clone()),
            active_local_profile_name: active_prof.map(|p| p.nickname.clone()),
            connection: self.sync.connection,
            pending_changes,
            last_sync_at_ms: self.sync.manifest.last_sync_at_ms,
            last_error: self.sync.last_error.clone(),
            last_conflict_notice: self.sync.last_conflict_notice.clone(),
        }
    }

    pub fn set_sync_in_flight(&mut self, in_flight: bool) {
        self.sync.sync_in_flight = in_flight;
        if self.sync.global.session.is_none() {
            self.sync.connection = SyncConnectionState::SignedOut;
        } else if in_flight {
            self.sync.connection = SyncConnectionState::Syncing;
        }
    }

    pub fn is_sync_in_flight(&self) -> bool {
        self.sync.sync_in_flight
    }

    pub fn last_dirty_at_ms(&self) -> Option<i64> {
        self.sync.last_dirty_at_ms
    }

    pub fn record_sync_failure(&mut self, offline: bool, message: String) {
        self.sync.sync_in_flight = false;
        self.sync.network_probed_offline = offline;
        self.sync.connection = if self.sync.global.session.is_none() {
            SyncConnectionState::SignedOut
        } else if offline {
            SyncConnectionState::Offline
        } else {
            SyncConnectionState::Error
        };
        if offline
            && self.is_viewing_cloud()
            && (self.sync.manifest.last_sync_at_ms.is_some()
                || self.sync.manifest.last_server_cursor > 0)
            && self.cloud_pending_changes_count() > 0
            && !self.sync.manifest.offline_edits_pending
        {
            self.sync.manifest.offline_edits_pending = true;
            self.sync_persist_manifest();
            self.save_current_web_bundle();
        }
        self.sync.last_error = Some(message);
    }

    pub fn update_active_session_tokens(&mut self, updated: ActiveAccountSession) {
        if self
            .sync
            .global
            .session
            .as_ref()
            .is_some_and(|cur| cur == &updated)
        {
            return;
        }
        self.sync.global.server_url = updated.server_url.clone();
        self.sync.global.session = Some(updated);
        self.sync_persist_global_state();
        self.save_current_web_bundle();
        self.bump_revision();
    }

    pub fn set_server_url_preference(&mut self, raw_url: &str) -> Result<String, String> {
        let normalized = normalize_server_url(raw_url).map_err(|e| e.to_string())?;
        self.sync.global.server_url = normalized.clone();
        if let Some(sess) = self.sync.global.session.as_mut() {
            sess.server_url = normalized.clone();
        }
        self.sync_persist_global_state();
        Ok(normalized)
    }

    /// Creates a new isolated local profile on this device.
    ///
    /// Created while signed in, the profile stays linked to that account (`owner_cloud_user_id`)
    /// so other cloud accounts on the same device never see it.
    pub fn create_local_profile(
        &mut self,
        nickname_raw: &str,
        switch_now: bool,
    ) -> Result<LocalProfile, String> {
        let nickname = sanitize_profile_nickname(nickname_raw);
        let now = now_unix_ms();
        let profile = LocalProfile {
            local_profile_id: Uuid::new_v4().to_string(),
            nickname,
            created_at_ms: now,
            last_used_at_ms: now,
            owner_cloud_user_id: self.sync.global.active_cloud_user_id().map(str::to_string),
        };
        self.sync.global.local_profiles.push(profile.clone());
        self.sync_persist_global_state();
        if switch_now {
            self.switch_to_local_profile(&profile.local_profile_id)?;
        }
        Ok(profile)
    }

    /// Renames an existing local profile without modifying its notebooks or ID.
    pub fn rename_local_profile(
        &mut self,
        profile_id: &str,
        nickname_raw: &str,
    ) -> Result<(), String> {
        let uid = self.sync.global.active_cloud_user_id().map(str::to_string);
        let Some(prof) = self.sync.global.local_profiles.iter_mut().find(|p| {
            p.local_profile_id == profile_id && p.is_accessible_to_cloud_user(uid.as_deref())
        }) else {
            return Err("local profile not found".to_string());
        };
        prof.nickname = sanitize_profile_nickname(nickname_raw);
        self.sync_persist_global_state();
        self.bump_revision();
        Ok(())
    }

    /// Deletes a local profile when more than one exists on this device.
    pub fn delete_local_profile(&mut self, profile_id: &str) -> Result<(), String> {
        if self.sync.global.accessible_local_profiles().len() <= 1 {
            return Err("at least one local profile must stay on this device".to_string());
        }
        let uid = self.sync.global.active_cloud_user_id();
        let Some(pos) =
            self.sync.global.local_profiles.iter().position(|p| {
                p.local_profile_id == profile_id && p.is_accessible_to_cloud_user(uid)
            })
        else {
            return Err("local profile not found".to_string());
        };
        let was_active_local =
            !self.is_viewing_cloud() && self.sync.global.active_local_profile_id == profile_id;
        self.sync.global.local_profiles.remove(pos);
        if self.sync.global.active_local_profile_id == profile_id {
            self.sync.global.active_local_profile_id = self
                .sync
                .global
                .accessible_local_profiles()
                .first()
                .map(|p| p.local_profile_id.clone())
                .unwrap_or_default();
        }
        self.sync.web_local_bundles.remove(profile_id);
        if let Some(ref root) = self.sync.desktop_root {
            let dir = profile_library_dir(root, profile_id);
            if dir.exists() {
                let _ = fs::remove_dir_all(dir);
            }
        }
        self.sync_persist_global_state();
        if was_active_local {
            let fallback_id = ensure_default_profile_in_global(&mut self.sync.global);
            self.load_local_profile_workspace(&fallback_id);
        } else {
            self.bump_revision();
        }
        Ok(())
    }

    /// Switches the active view to a local profile while keeping any connected cloud session intact.
    pub fn switch_to_local_profile(&mut self, profile_id: &str) -> Result<(), String> {
        let uid = self.sync.global.active_cloud_user_id();
        if !self
            .sync
            .global
            .local_profiles
            .iter()
            .any(|p| p.local_profile_id == profile_id && p.is_accessible_to_cloud_user(uid))
        {
            return Err("local profile not found".to_string());
        }
        self.flush_active_workspace_to_storage();
        if let Some(prof) = self
            .sync
            .global
            .local_profiles
            .iter_mut()
            .find(|p| p.local_profile_id == profile_id)
        {
            prof.last_used_at_ms = now_unix_ms();
        }
        self.sync.global.active_local_profile_id = profile_id.to_string();
        self.sync.global.active_workspace =
            Some(ActiveWorkspace::LocalProfile(profile_id.to_string()));
        self.sync_persist_global_state();
        self.load_local_profile_workspace(profile_id);
        Ok(())
    }

    /// Switches the active view back to the currently connected cloud account.
    pub fn switch_to_cloud_workspace(&mut self) -> Result<(), String> {
        let Some(acct) = self.sync.global.session.clone() else {
            return Err("no cloud account is connected".to_string());
        };
        if self.is_viewing_cloud() {
            return Ok(());
        }
        self.switch_to_authenticated_library(acct, AuthWorkspaceMode::SwitchOnly);
        Ok(())
    }

    /// Switches to the authenticated user's isolated account library, optionally promoting or merging
    /// a chosen local profile without creating duplicate copies.
    pub fn switch_to_authenticated_library(
        &mut self,
        auth: ActiveAccountSession,
        mode: AuthWorkspaceMode,
    ) {
        self.flush_active_workspace_to_storage();
        let captured_source = match &mode {
            AuthWorkspaceMode::SwitchOnly => None,
            AuthWorkspaceMode::PromoteLocalProfile { source_profile_id }
            | AuthWorkspaceMode::MergeLocalProfile { source_profile_id } => {
                self.take_local_profile_payload(source_profile_id, &auth.user.user_id)
            }
        };

        self.sync.global.server_url = auth.server_url.clone();
        self.sync.global.device_name = auth.device_name.clone();
        self.sync.global.session = Some(auth.clone());
        self.sync.global.active_workspace =
            Some(ActiveWorkspace::CloudUser(auth.user.user_id.clone()));
        self.sync.last_error = None;
        self.sync.last_conflict_notice = None;
        self.sync_persist_global_state();

        let owner = OwnerId::CloudUser(auth.user.user_id.clone());
        if let Some(root) = self.sync.desktop_root.clone() {
            let acct_dir = account_library_dir(&root, &auth.user.user_id);
            let (next_session, is_placeholder) =
                open_or_merge_account_dir(&acct_dir, owner.clone(), captured_source);

            let keep_root = self.sync.desktop_root.clone();
            let keep_global = self.sync.global.clone();
            *self = next_session;
            self.sync.desktop_root = keep_root;
            self.sync.track_manifest = true;
            self.sync.global = keep_global;
            self.sync.connection = SyncConnectionState::Offline;
            self.sync_init_from_disk(is_placeholder, owner);
            self.sync_reconcile_slots();
            self.bump_revision();
            return;
        }

        if self.sync.web_cache_enabled {
            self.switch_web_authenticated_library(&auth, owner, captured_source);
            return;
        }

        self.sync.manifest.owner_id = Some(owner);
        self.sync.connection = SyncConnectionState::Offline;
        self.bump_revision();
    }

    fn switch_web_authenticated_library(
        &mut self,
        auth: &ActiveAccountSession,
        owner: OwnerId,
        captured_source: Option<CapturedProfileData>,
    ) {
        let existing = self
            .sync
            .web_account_bundles
            .get(&auth.user.user_id)
            .cloned();
        let mut next_session = match existing {
            Some(bundle) => restore_web_bundle(&bundle),
            None => restore_web_bundle(&placeholder_web_bundle()),
        };
        if let Some(source) = captured_source {
            merge_captured_into_session(&mut next_session, source, owner.clone(), None);
        } else {
            next_session.sync.manifest.owner_id = Some(owner);
        }
        next_session.sync.global = self.sync.global.clone();
        next_session.sync.web_cache_enabled = true;
        next_session.sync.track_manifest = true;
        next_session.sync.web_local_bundles = self.sync.web_local_bundles.clone();
        next_session.sync.web_account_bundles = self.sync.web_account_bundles.clone();
        next_session.sync.connection = SyncConnectionState::Offline;
        *self = next_session;
        self.sync_reconcile_slots();
        self.bump_revision();
    }

    /// Flushes account data safely and signs out back into the active local profile.
    pub fn sign_out_to_local_library(&mut self) {
        self.flush_active_workspace_to_storage();
        self.sync.global.session = None;
        if self.sync.global.accessible_local_profiles().is_empty() {
            self.sync.global.needs_initial_nickname = true;
        }
        let target_id = ensure_default_profile_in_global(&mut self.sync.global);
        self.sync.global.active_workspace = Some(ActiveWorkspace::LocalProfile(target_id.clone()));
        self.sync.last_error = None;
        self.sync.last_conflict_notice = None;
        self.sync.sync_in_flight = false;
        self.sync_persist_global_state();
        self.load_local_profile_workspace(&target_id);
    }

    /// Moves all notebooks, trashed items, folders, and preferences from `source_profile_id`
    /// into the connected synced account (renaming notebooks to `<title> - <nickname>`),
    /// then deletes all data from the old local account.
    pub fn migrate_local_account_to_cloud(
        &mut self,
        source_profile_id: &str,
    ) -> Result<LocalMigrationSummary, String> {
        let Some(acct) = self.sync.global.session.clone() else {
            return Err(
                "Create and sign in to a synced account before migrating local data.".to_string(),
            );
        };
        let Some(profile) = self
            .sync
            .global
            .local_profiles
            .iter()
            .find(|p| p.local_profile_id == source_profile_id)
            .cloned()
        else {
            return Err("Local account not found on this device.".to_string());
        };
        if !profile.is_accessible_to_cloud_user(Some(&acct.user.user_id)) {
            return Err("This local account belongs to a different cloud account.".to_string());
        }

        self.flush_active_workspace_to_storage();
        let captured = self.read_local_profile_payload(source_profile_id);

        if !self.is_viewing_cloud() {
            self.switch_to_cloud_workspace()?;
        }

        let existing_active = self
            .notebooks
            .iter()
            .enumerate()
            .filter(|(idx, _)| {
                !self
                    .sync
                    .slot_placeholder_blank
                    .get(*idx)
                    .copied()
                    .unwrap_or(false)
            })
            .count();
        if existing_active.saturating_add(captured.docs.len()) > MAX_NOTEBOOKS {
            return Err(format!(
                "Synced account cannot hold {} additional notebooks cleanly (limit {MAX_NOTEBOOKS}).",
                captured.docs.len()
            ));
        }

        let moved_notebooks = captured.docs.len();
        let moved_trash = captured.trash.len();
        for mut doc in captured.docs {
            doc.title = rename_migrated_notebook_title(&doc.title, &profile.nickname);
            if doc.id.is_empty() {
                doc.id = Uuid::new_v4().to_string();
            }
            doc.updated_at_ms = doc.updated_at_ms.max(now_unix_ms());
            doc.meta_updated_at_ms = doc.meta_updated_at_ms.max(doc.updated_at_ms);
            self.insert_transferred_doc_into_active_cloud(doc, &acct.user.user_id);
        }
        for mut entry in captured.trash {
            entry.doc.title = rename_migrated_notebook_title(&entry.doc.title, &profile.nickname);
            let _ = self.add_trash_entry_persisted(entry.doc, entry.deleted_at_ms);
        }
        if moved_notebooks > 0 || !captured.prefs.library_folders.is_empty() {
            merge_studio_prefs(&mut self.prefs, captured.prefs, false);
            let _ = self.save_prefs();
            self.sync_mark_prefs_dirty();
        }
        self.sync_reconcile_slots();
        let _ = self.save_library();
        self.sync_persist_manifest();
        self.save_current_web_bundle();

        // Only after all moved items are safely persisted in the synced account do we delete
        // the old local account and its storage.
        self.remove_local_profile_storage(source_profile_id);
        self.sync.global.needs_initial_nickname = false;
        self.sync_persist_global_state();
        self.bump_revision();

        let target_account_name = if acct.user.display_name.trim().is_empty() {
            acct.user.username
        } else {
            acct.user.display_name
        };
        Ok(LocalMigrationSummary {
            source_profile_id: source_profile_id.to_string(),
            source_nickname: profile.nickname,
            target_account_name,
            moved_notebooks,
            moved_trash,
        })
    }

    fn count_local_profile_notebooks(&self, profile_id: &str) -> usize {
        if !self.is_viewing_cloud()
            && self
                .active_local_profile()
                .is_some_and(|p| p.local_profile_id == profile_id)
        {
            return self
                .notebooks
                .iter()
                .enumerate()
                .filter(|(idx, _)| {
                    !self
                        .sync
                        .slot_placeholder_blank
                        .get(*idx)
                        .copied()
                        .unwrap_or(false)
                })
                .count();
        }
        if let Some(ref root) = self.sync.desktop_root {
            let dir = profile_library_dir(root, profile_id);
            return count_ncraft_files_in_dir(&dir);
        }
        if self.sync.web_cache_enabled
            && let Some(bundle) = self.sync.web_local_bundles.get(profile_id)
        {
            return bundle
                .notebooks
                .iter()
                .enumerate()
                .filter(|(idx, _)| {
                    !bundle
                        .slot_placeholder_blank
                        .get(*idx)
                        .copied()
                        .unwrap_or(false)
                })
                .count();
        }
        0
    }

    /// Lists notebooks inside a local profile on this device (for manual Cloud import/transfer).
    pub fn list_local_profile_notebooks(&self, profile_id: &str) -> Vec<LocalNotebookEntry> {
        let uid = self.sync.global.active_cloud_user_id();
        if !self
            .sync
            .global
            .local_profiles
            .iter()
            .any(|p| p.local_profile_id == profile_id && p.is_accessible_to_cloud_user(uid))
        {
            return Vec::new();
        }
        if !self.is_viewing_cloud()
            && self
                .active_local_profile()
                .is_some_and(|p| p.local_profile_id == profile_id)
        {
            return self
                .notebooks
                .iter()
                .enumerate()
                .filter(|(idx, _)| {
                    !self
                        .sync
                        .slot_placeholder_blank
                        .get(*idx)
                        .copied()
                        .unwrap_or(false)
                })
                .map(|(idx, doc)| LocalNotebookEntry {
                    notebook_id: self
                        .sync
                        .slot_ids
                        .get(idx)
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .unwrap_or_else(|| doc.id.clone()),
                    title: doc.title.clone(),
                    page_count: doc.page_count(),
                    folder_path: doc.folder_path.clone(),
                })
                .collect();
        }
        if let Some(ref root) = self.sync.desktop_root {
            let dir = profile_library_dir(root, profile_id);
            let (s, is_placeholder) =
                Self::open_library_mode(dir, LibraryInitMode::BlankPlaceholderOnly);
            if is_placeholder {
                return Vec::new();
            }
            return s
                .notebooks
                .iter()
                .map(|doc| LocalNotebookEntry {
                    notebook_id: doc.id.clone(),
                    title: doc.title.clone(),
                    page_count: doc.page_count(),
                    folder_path: doc.folder_path.clone(),
                })
                .collect();
        }
        if self.sync.web_cache_enabled
            && let Some(bundle) = self.sync.web_local_bundles.get(profile_id)
        {
            return bundle
                .notebooks
                .iter()
                .enumerate()
                .filter(|(idx, _)| {
                    !bundle
                        .slot_placeholder_blank
                        .get(*idx)
                        .copied()
                        .unwrap_or(false)
                })
                .map(|(_, doc)| LocalNotebookEntry {
                    notebook_id: doc.id.clone(),
                    title: doc.title.clone(),
                    page_count: doc.page_count(),
                    folder_path: doc.folder_path.clone(),
                })
                .collect();
        }
        Vec::new()
    }

    /// Manually moves or copies an individual notebook from `source_profile_id` into the connected
    /// cloud account (`PendingPush`). Moving preserves UUID; copying mints a fresh UUID.
    pub fn transfer_local_notebook_to_cloud(
        &mut self,
        source_profile_id: &str,
        notebook_id: &str,
        move_notebook: bool,
    ) -> Result<String, String> {
        let Some(acct) = self.sync.global.session.clone() else {
            return Err("sign in to a cloud account first".to_string());
        };
        let Some(profile) = self
            .sync
            .global
            .local_profiles
            .iter()
            .find(|p| p.local_profile_id == source_profile_id)
        else {
            return Err("local profile not found".to_string());
        };
        if !profile.is_accessible_to_cloud_user(Some(&acct.user.user_id)) {
            return Err("this local profile belongs to a different cloud account".to_string());
        }
        self.flush_active_workspace_to_storage();
        let viewing_cloud = self.is_viewing_cloud();
        let viewing_source = !viewing_cloud
            && self
                .active_local_profile()
                .is_some_and(|p| p.local_profile_id == source_profile_id);

        let mut extracted =
            self.extract_local_notebook(source_profile_id, notebook_id, move_notebook)?;
        if !move_notebook || extracted.id.is_empty() {
            extracted.id = Uuid::new_v4().to_string();
        }
        let target_id = extracted.id.clone();

        if viewing_cloud {
            self.insert_transferred_doc_into_active_cloud(extracted, &acct.user.user_id);
        } else {
            self.write_transferred_doc_into_cloud_storage(extracted, &acct.user.user_id)?;
        }
        if viewing_source && move_notebook {
            self.load_local_profile_workspace(source_profile_id);
        }
        self.bump_revision();
        Ok(target_id)
    }

    fn flush_active_workspace_to_storage(&mut self) {
        self.sync_active_notebook_slot();
        self.sync_reconcile_folders();
        let _ = self.save_library();
        let _ = self.save_prefs();
        self.sync_persist_manifest();
        self.save_current_web_bundle();
    }

    fn load_local_profile_workspace(&mut self, profile_id: &str) {
        let owner = OwnerId::LocalProfile(profile_id.to_string());
        let conn = if self.sync.global.session.is_some() {
            SyncConnectionState::Offline
        } else {
            SyncConnectionState::SignedOut
        };
        if let Some(root) = self.sync.desktop_root.clone() {
            let keep_global = self.sync.global.clone();
            let prof_dir = profile_library_dir(&root, profile_id);
            let (next_session, is_placeholder) =
                Self::open_library_mode(prof_dir, LibraryInitMode::SeedStartersIfFresh);
            *self = next_session;
            self.sync.desktop_root = Some(root);
            self.sync.track_manifest = true;
            self.sync.global = keep_global;
            self.sync.connection = conn;
            self.sync_init_from_disk(is_placeholder, owner);
            self.bump_revision();
            return;
        }
        if self.sync.web_cache_enabled {
            let bundle = self
                .sync
                .web_local_bundles
                .get(profile_id)
                .cloned()
                .unwrap_or_else(starter_web_bundle);
            let mut next = restore_web_bundle(&bundle);
            next.sync.global = self.sync.global.clone();
            next.sync.web_cache_enabled = true;
            next.sync.track_manifest = true;
            next.sync.web_local_bundles = self.sync.web_local_bundles.clone();
            next.sync.web_account_bundles = self.sync.web_account_bundles.clone();
            next.sync.connection = conn;
            next.sync.manifest.owner_id = Some(owner);
            *self = next;
            self.sync_reconcile_slots();
            self.bump_revision();
            return;
        }
        self.sync.manifest.owner_id = Some(owner);
        self.sync.connection = conn;
        self.bump_revision();
    }

    fn read_local_profile_payload(&mut self, source_profile_id: &str) -> CapturedProfileData {
        let mut docs = Vec::new();
        let mut trash = Vec::new();
        let mut prefs = UserStudioConfig::default();

        if let Some(ref root) = self.sync.desktop_root {
            let src_dir = profile_library_dir(root, source_profile_id);
            let (mut src_sess, is_placeholder) =
                Self::open_library_mode(src_dir, LibraryInitMode::BlankPlaceholderOnly);
            if !is_placeholder {
                src_sess.sync_reconcile_slots();
                docs = src_sess.notebooks.iter().map(|d| (**d).clone()).collect();
            }
            trash = src_sess
                .trashed_notebooks
                .iter()
                .map(|entry| CachedTrashEntry {
                    doc: (*entry.doc).clone(),
                    deleted_at_ms: entry.deleted_at_ms,
                })
                .collect();
            prefs = src_sess.prefs.clone();
        } else if self.sync.web_cache_enabled
            && let Some(bundle) = self.sync.web_local_bundles.get(source_profile_id)
        {
            for (idx, doc) in bundle.notebooks.iter().enumerate() {
                if bundle
                    .slot_placeholder_blank
                    .get(idx)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                let mut d = doc.clone();
                if d.id.is_empty() {
                    d.id = Uuid::new_v4().to_string();
                }
                docs.push(d);
            }
            trash = bundle.trash.clone();
            prefs = bundle.prefs.clone();
        }

        CapturedProfileData { docs, trash, prefs }
    }

    fn remove_local_profile_storage(&mut self, source_profile_id: &str) {
        if let Some(pos) = self
            .sync
            .global
            .local_profiles
            .iter()
            .position(|p| p.local_profile_id == source_profile_id)
        {
            self.sync.global.local_profiles.remove(pos);
        }
        if self.sync.global.active_local_profile_id == source_profile_id {
            self.sync.global.active_local_profile_id = self
                .sync
                .global
                .local_profiles
                .first()
                .map(|p| p.local_profile_id.clone())
                .unwrap_or_default();
        }
        self.sync.web_local_bundles.remove(source_profile_id);
        if let Some(ref root) = self.sync.desktop_root {
            let src_dir = profile_library_dir(root, source_profile_id);
            if src_dir.exists() {
                let _ = fs::remove_dir_all(&src_dir);
            }
        }
    }

    fn take_local_profile_payload(
        &mut self,
        source_profile_id: &str,
        target_cloud_user_id: &str,
    ) -> Option<CapturedProfileData> {
        let accessible = self.sync.global.local_profiles.iter().any(|p| {
            p.local_profile_id == source_profile_id
                && p.is_accessible_to_cloud_user(Some(target_cloud_user_id))
        });
        if !accessible {
            return None;
        }
        let payload = self.read_local_profile_payload(source_profile_id);
        self.remove_local_profile_storage(source_profile_id);
        Some(payload)
    }

    fn extract_local_notebook(
        &mut self,
        source_profile_id: &str,
        notebook_id: &str,
        remove_from_source: bool,
    ) -> Result<Document, String> {
        if let Some(ref root) = self.sync.desktop_root {
            let src_dir = profile_library_dir(root, source_profile_id);
            let (mut src_sess, is_placeholder) =
                Self::open_library_mode(src_dir.clone(), LibraryInitMode::BlankPlaceholderOnly);
            if is_placeholder {
                return Err("source local profile has no notebooks".to_string());
            }
            src_sess.sync.track_manifest = true;
            src_sess
                .sync_init_from_disk(false, OwnerId::LocalProfile(source_profile_id.to_string()));
            let Some(idx) = src_sess.find_slot_by_id(notebook_id) else {
                return Err("notebook not found in source local profile".to_string());
            };
            let doc = (*src_sess.notebooks[idx]).clone();
            if remove_from_source {
                src_sess.notebooks.remove(idx);
                src_sess.library.remove_slot(idx);
                src_sess.sync.slot_ids.remove(idx);
                src_sess.sync.slot_dirty_hash.remove(idx);
                src_sess.sync.slot_placeholder_blank.remove(idx);
                src_sess
                    .sync
                    .manifest
                    .notebooks
                    .retain(|m| m.notebook_id != notebook_id);
                let _ = save_manifest(&src_dir, &src_sess.sync.manifest);
            }
            return Ok(doc);
        }

        if self.sync.web_cache_enabled
            && let Some(bundle) = self.sync.web_local_bundles.get_mut(source_profile_id)
        {
            let pos = bundle
                .notebooks
                .iter()
                .enumerate()
                .position(|(i, d)| {
                    d.id == notebook_id
                        || bundle.slot_ids.get(i).is_some_and(|id| id == notebook_id)
                })
                .ok_or_else(|| "notebook not found in source local profile".to_string())?;
            let doc = if remove_from_source {
                let d = bundle.notebooks.remove(pos);
                if pos < bundle.slot_ids.len() {
                    bundle.slot_ids.remove(pos);
                }
                if pos < bundle.slot_dirty_hash.len() {
                    bundle.slot_dirty_hash.remove(pos);
                }
                if pos < bundle.slot_placeholder_blank.len() {
                    bundle.slot_placeholder_blank.remove(pos);
                }
                bundle
                    .manifest
                    .notebooks
                    .retain(|m| m.notebook_id != notebook_id);
                d
            } else {
                bundle.notebooks[pos].clone()
            };
            return Ok(doc);
        }
        Err("local profile storage unavailable".to_string())
    }

    fn insert_transferred_doc_into_active_cloud(&mut self, mut doc: Document, user_id: &str) {
        doc.sync_active_to_pages();
        let hash = canonical_document_hash(&doc);
        let id = doc.id.clone();
        if !doc.folder_path.is_empty() {
            let mut folders = self.prefs.library_folders.clone();
            folders.push(doc.folder_path.clone());
            self.prefs.library_folders = sanitize_library_folders(&folders);
            let _ = self.save_prefs();
            self.sync_mark_prefs_dirty();
        }
        if let Some(existing_idx) = self.find_slot_by_id(&id) {
            self.notebooks[existing_idx] = Arc::new(doc);
            self.library.mark_dirty(existing_idx, self.notebooks.len());
            self.sync.slot_dirty_hash[existing_idx] = true;
            self.sync.slot_placeholder_blank[existing_idx] = false;
        } else if let Some(p_idx) = self.sync.slot_placeholder_blank.iter().position(|&b| b) {
            self.notebooks[p_idx] = Arc::new(doc);
            self.library.mark_dirty(p_idx, self.notebooks.len());
            self.sync.slot_ids[p_idx] = id.clone();
            self.sync.slot_dirty_hash[p_idx] = true;
            self.sync.slot_placeholder_blank[p_idx] = false;
            if p_idx == self.active_notebook {
                self.doc = Arc::clone(&self.notebooks[p_idx]);
            }
        } else if self.notebooks.len() < MAX_NOTEBOOKS {
            self.notebooks.push(Arc::new(doc));
            self.library.push_slot(None, true, self.notebooks.len());
            self.sync.slot_ids.push(id.clone());
            self.sync.slot_dirty_hash.push(true);
            self.sync.slot_placeholder_blank.push(false);
        }
        self.sync.manifest.owner_id = Some(OwnerId::CloudUser(user_id.to_string()));
        self.sync.manifest.mark_notebook_dirty(&id, &hash);
        self.sync_reconcile_folders();
        self.sync.last_dirty_at_ms = Some(now_unix_ms());
        let _ = self.save_library();
        self.sync_persist_manifest();
    }

    fn write_transferred_doc_into_cloud_storage(
        &mut self,
        mut doc: Document,
        user_id: &str,
    ) -> Result<(), String> {
        doc.sync_active_to_pages();
        let hash = canonical_document_hash(&doc);
        let id = doc.id.clone();
        if let Some(ref root) = self.sync.desktop_root {
            let acct_dir = account_library_dir(root, user_id);
            let _ = fs::create_dir_all(&acct_dir);
            let mut manifest = load_manifest(&acct_dir);
            manifest.owner_id = Some(OwnerId::CloudUser(user_id.to_string()));
            let target_path = manifest
                .meta(&id)
                .and_then(|m| m.local_file.as_ref())
                .map(|name| acct_dir.join(name))
                .unwrap_or_else(|| acct_dir.join(format!("nb_{id}.dcraft")));
            write_document_snapshot(&target_path, &doc)?;
            let _ = fs::write(acct_dir.join(LIBRARY_MARKER_FILE), b"1");
            manifest.mark_notebook_dirty(&id, &hash);
            if let Some(name) = target_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
            {
                manifest.bind_local_file(&id, &name);
            }
            save_manifest(&acct_dir, &manifest).map_err(|e| e.to_string())?;
            return Ok(());
        }
        if self.sync.web_cache_enabled {
            let bundle = self
                .sync
                .web_account_bundles
                .entry(user_id.to_string())
                .or_insert_with(placeholder_web_bundle);
            let mut sess = restore_web_bundle(bundle);
            sess.sync.manifest.owner_id = Some(OwnerId::CloudUser(user_id.to_string()));
            sess.insert_transferred_doc_into_active_cloud(doc, user_id);
            *bundle = sess.capture_web_bundle();
            return Ok(());
        }
        Ok(())
    }

    /// Captures a lock-free snapshot of local dirty notebooks, folders, and tombstones for HTTP sync.
    pub fn prepare_sync_batch(&mut self) -> Option<SyncBatch> {
        let _ = self.sync.global.session.as_ref()?;
        if !self.is_viewing_cloud() {
            return None;
        }
        self.sync_active_notebook_slot();
        self.sync_reconcile_slots();
        self.sync_reconcile_folders();

        let count = self.notebooks.len();
        self.ensure_sync_vec_lens(count);

        let mut local_docs = Vec::new();
        for idx in 0..count {
            if self.sync.slot_placeholder_blank[idx] {
                continue;
            }
            let id = self.sync.slot_ids[idx].clone();
            let slot_dirty = self.sync.slot_dirty_hash[idx];
            let needs_check = slot_dirty
                || self
                    .sync
                    .manifest
                    .meta(&id)
                    .is_none_or(|m| m.cloud_dirty || m.server_version == 0);
            if !needs_check {
                continue;
            }

            let doc_arc = &self.notebooks[idx];
            let content_hash = canonical_document_hash(doc_arc);
            if slot_dirty || self.sync.manifest.meta(&id).is_none() {
                self.sync.manifest.mark_notebook_dirty(&id, &content_hash);
                self.sync.slot_dirty_hash[idx] = false;
            }

            if let Some(meta) = self.sync.manifest.meta(&id)
                && (meta.cloud_dirty || meta.server_version == 0)
            {
                local_docs.push(LocalNotebookSnapshot {
                    notebook_id: id,
                    base_version: meta.server_version,
                    cloud_dirty: true,
                    content_hash,
                    doc: (**doc_arc).clone(),
                });
            }
        }

        let folder_changes: Vec<LocalFolderChange> = self
            .sync
            .manifest
            .folders
            .iter()
            .filter(|f| f.cloud_dirty || f.server_version == 0 || f.deleted_at_ms.is_some())
            .map(|f| LocalFolderChange {
                folder_id: f.folder_id.clone(),
                base_version: f.server_version,
                path: f.path.clone(),
                color_hex: f.color_hex.clone(),
                deleted: f.deleted_at_ms.is_some(),
                updated_at_ms: f.updated_at_ms,
                deleted_at_ms: f.deleted_at_ms,
            })
            .collect();

        if self.sync.prefs_dirty_pending {
            let hash = canonical_prefs_hash(&self.prefs);
            self.sync.manifest.mark_prefs_dirty(&hash);
            self.sync.prefs_dirty_pending = false;
        }

        let prefs_push = if self.sync.manifest.prefs_dirty {
            Some(LocalPrefsPush {
                base_version: self.sync.manifest.prefs_version,
                content_hash: canonical_prefs_hash(&self.prefs),
                prefs: self.prefs.clone(),
            })
        } else {
            None
        };

        self.sync_persist_manifest();

        Some(SyncBatch {
            client_cursor: self.sync.manifest.last_server_cursor,
            device_name: self.sync.global.device_name.clone(),
            local_docs,
            tombstones: self.sync.manifest.tombstones.clone(),
            folder_changes,
            prefs_push,
            force_overwrite: false,
        })
    }

    /// Captures a complete authoritative snapshot of the active cloud workspace for
    /// "Keep Local Version" conflict resolution (overwriting and pruning unchosen server data).
    pub fn prepare_force_local_sync_batch(&mut self) -> Option<SyncBatch> {
        let _ = self.sync.global.session.as_ref()?;
        if !self.is_viewing_cloud() {
            return None;
        }
        self.sync_active_notebook_slot();
        self.sync_reconcile_slots();
        self.sync_reconcile_folders();

        let count = self.notebooks.len();
        self.ensure_sync_vec_lens(count);

        let mut local_docs = Vec::new();
        for idx in 0..count {
            if self.sync.slot_placeholder_blank[idx] {
                continue;
            }
            let id = self.sync.slot_ids[idx].clone();
            let doc_arc = &self.notebooks[idx];
            let content_hash = canonical_document_hash(doc_arc);
            self.sync.manifest.mark_notebook_dirty(&id, &content_hash);
            self.sync.slot_dirty_hash[idx] = false;
            let base_version = self.sync.manifest.meta(&id).map_or(0, |m| m.server_version);
            local_docs.push(LocalNotebookSnapshot {
                notebook_id: id,
                base_version,
                cloud_dirty: true,
                content_hash,
                doc: (**doc_arc).clone(),
            });
        }

        let folder_changes: Vec<LocalFolderChange> = self
            .sync
            .manifest
            .folders
            .iter()
            .filter(|f| f.deleted_at_ms.is_none())
            .map(|f| LocalFolderChange {
                folder_id: f.folder_id.clone(),
                base_version: f.server_version,
                path: f.path.clone(),
                color_hex: f.color_hex.clone(),
                deleted: false,
                updated_at_ms: f.updated_at_ms.max(now_unix_ms()),
                deleted_at_ms: None,
            })
            .collect();

        let prefs_push = Some(LocalPrefsPush {
            base_version: self.sync.manifest.prefs_version,
            content_hash: canonical_prefs_hash(&self.prefs),
            prefs: self.prefs.clone(),
        });

        Some(SyncBatch {
            client_cursor: self.sync.manifest.last_server_cursor,
            device_name: self.sync.global.device_name.clone(),
            local_docs,
            tombstones: self.sync.manifest.tombstones.clone(),
            folder_changes,
            prefs_push,
            force_overwrite: true,
        })
    }

    /// Replaces the entire local cloud workspace with the authoritative Server Version,
    /// permanently discarding unchosen local edits, additions, trash, and folders.
    pub fn apply_keep_server_version(&mut self, outcome: SyncCycleOutcome) -> Result<(), String> {
        if self.live_stroke.is_some() || self.live_eraser.is_some() {
            return Err("cannot replace workspace during an active stroke".to_string());
        }
        if !self.is_viewing_cloud() {
            return Err(
                "switch to the synced account before keeping the server version".to_string(),
            );
        }

        if let Some(refreshed) = outcome.refreshed_session.clone() {
            self.sync.global.server_url = refreshed.server_url.clone();
            self.sync.global.session = Some(refreshed);
            self.sync_persist_global_state();
        } else if let Some(sess) = self.sync.global.session.as_mut()
            && sess.user != outcome.server_user
        {
            sess.user = outcome.server_user.clone();
            self.sync_persist_global_state();
        }

        // Delete all unchosen local cloud notebook files and local trash files.
        for path in self.library.slot_files.drain(..).flatten() {
            let _ = fs::remove_file(path);
        }
        self.clear_trash_persisted();

        let owner = self.sync.manifest.owner_id.clone();
        self.sync.manifest = SyncManifest {
            owner_id: owner,
            last_server_cursor: outcome.new_cursor,
            last_sync_at_ms: Some(outcome.synced_at_ms),
            offline_edits_pending: false,
            ..Default::default()
        };

        if let Some(ref rp) = outcome.updated_prefs {
            self.prefs = rp.prefs.clone().sanitized();
            self.sync.manifest.prefs_version = rp.version;
            self.sync.manifest.prefs_hash = rp.content_hash.clone();
        } else {
            self.prefs = UserStudioConfig::default();
        }
        self.prefs.library_folders.clear();
        self.apply_pen_size_preferences();
        self.sync.prefs_dirty_pending = false;
        self.sync.manifest.prefs_dirty = false;

        self.notebooks.clear();
        self.sync.slot_ids.clear();
        self.sync.slot_dirty_hash.clear();
        self.sync.slot_placeholder_blank.clear();
        self.library.sync_len(0);

        let mut dummy_notices = Vec::new();
        for update in &outcome.downloaded_updates {
            if !update.deleted {
                let _ = self.apply_remote_upsert(update, outcome.synced_at_ms, &mut dummy_notices);
            }
        }
        let _ = self.apply_remote_folders(&outcome.downloaded_folders);

        if self.notebooks.is_empty() {
            let mut blank = Document::default();
            blank.sync_active_to_pages();
            self.notebooks.push(Arc::new(blank));
            self.library.sync_len(1);
            self.sync.slot_ids.push(String::new());
            self.sync.slot_dirty_hash.push(false);
            self.sync.slot_placeholder_blank.push(true);
        }

        self.active_notebook = 0;
        self.doc = Arc::clone(&self.notebooks[0]);
        self.history.clear();
        if let Some(dir) = self.library.dir.clone() {
            let _ = fs::write(dir.join(LIBRARY_MARKER_FILE), b"1");
        }
        self.sync.manifest.offline_edits_pending = false;
        self.sync.network_probed_offline = false;
        self.sync.sync_in_flight = false;
        self.sync.connection = SyncConnectionState::Synced;
        self.sync.last_error = None;
        self.sync.last_conflict_notice = None;
        self.sync.last_dirty_at_ms = None;

        let _ = self.save_library();
        let _ = self.save_prefs();
        self.sync_persist_manifest();
        self.save_current_web_bundle();
        self.bump_revision();
        Ok(())
    }

    /// Applies the completed server sync outcome onto the live Session (must not have an active stroke).
    pub fn apply_sync_outcome(&mut self, outcome: SyncCycleOutcome) -> Result<(), String> {
        if self.live_stroke.is_some() || self.live_eraser.is_some() {
            return Err("cannot apply sync outcome during an active stroke".to_string());
        }
        if !self.is_viewing_cloud() {
            return Err(
                "cannot apply cloud sync outcome while viewing a local profile".to_string(),
            );
        }
        self.sync_active_notebook_slot();

        if let Some(refreshed) = outcome.refreshed_session {
            self.sync.global.server_url = refreshed.server_url.clone();
            self.sync.global.session = Some(refreshed);
            self.sync_persist_global_state();
        } else if let Some(sess) = self.sync.global.session.as_mut()
            && sess.user != outcome.server_user
        {
            sess.user = outcome.server_user.clone();
            self.sync_persist_global_state();
        }

        self.sync.manifest.last_server_cursor = outcome.new_cursor;
        self.ack_accepted_uploads(&outcome.accepted_uploads, outcome.synced_at_ms);
        self.ack_accepted_folders(&outcome.accepted_folders);

        for deleted_id in &outcome.cleared_tombstones {
            self.sync.manifest.clear_tombstone(deleted_id);
        }

        let mut notice_titles = Vec::new();
        for conflict in &outcome.resolved_conflicts {
            notice_titles.push(conflict.conflict_title.clone());
            self.insert_conflict_copy_slot(
                &conflict.original_notebook_id,
                &conflict.conflict_notebook_id,
                conflict.local_doc.clone(),
                &conflict.local_hash,
            );
        }

        let mut updated_any_doc = false;
        for update in &outcome.downloaded_updates {
            if !update.deleted
                && self.apply_remote_upsert(update, outcome.synced_at_ms, &mut notice_titles)
            {
                updated_any_doc = true;
            }
        }
        for update in &outcome.downloaded_updates {
            if update.deleted && self.apply_remote_delete(&update.notebook_id) {
                updated_any_doc = true;
            }
        }
        if self.apply_remote_folders(&outcome.downloaded_folders) {
            updated_any_doc = true;
        }

        if outcome.prefs_conflict.is_some() {
            let cur_hash = canonical_prefs_hash(&self.prefs);
            self.sync.manifest.mark_prefs_dirty(&cur_hash);
        } else if let Some((ver, ref sent_hash)) = outcome.ack_prefs {
            let cur_hash = canonical_prefs_hash(&self.prefs);
            self.sync.manifest.prefs_version = ver;
            self.sync.manifest.prefs_hash = sent_hash.clone();
            self.sync.manifest.prefs_dirty =
                self.sync.prefs_dirty_pending || cur_hash != *sent_hash;
        }
        if let Some(ref rp) = outcome.updated_prefs
            && !self.sync.manifest.prefs_dirty
            && !self.sync.prefs_dirty_pending
        {
            self.apply_remote_prefs_preserving_folders(&rp.prefs);
            self.sync.manifest.prefs_version = rp.version;
            self.sync.manifest.prefs_hash = rp.content_hash.clone();
            self.sync.manifest.prefs_dirty = false;
            let _ = self.save_prefs();
            updated_any_doc = true;
        }

        if (!outcome.downloaded_updates.is_empty() || !outcome.accepted_uploads.is_empty())
            && let Some(dir) = self.library.dir.clone()
        {
            let _ = fs::write(dir.join(LIBRARY_MARKER_FILE), b"1");
        }

        self.sync.manifest.last_sync_at_ms = Some(outcome.synced_at_ms);
        self.sync.manifest.offline_edits_pending = false;
        self.sync.network_probed_offline = false;
        self.sync.sync_in_flight = false;
        self.sync.connection = SyncConnectionState::Synced;
        self.sync.last_error = None;
        self.sync.last_conflict_notice = if notice_titles.is_empty() {
            None
        } else {
            Some(format!(
                "Preserved offline conflict copy: {}",
                notice_titles.join(", ")
            ))
        };
        if self.compute_pending_changes_fast() == 0 {
            self.sync.last_dirty_at_ms = None;
        }

        let _ = self.save_library();
        self.sync_persist_manifest();
        if updated_any_doc || !notice_titles.is_empty() {
            self.bump_revision();
        }
        Ok(())
    }

    pub(crate) fn sync_mark_slot_dirty(&mut self, index: usize) {
        if !self.sync.track_manifest {
            return;
        }
        self.ensure_sync_vec_lens(self.notebooks.len());
        if index < self.sync.slot_dirty_hash.len() {
            self.sync.slot_dirty_hash[index] = true;
            self.sync.slot_placeholder_blank[index] = false;
            self.sync.last_dirty_at_ms.get_or_insert_with(now_unix_ms);
            self.mark_offline_edits_if_disconnected();
        }
    }

    pub(crate) fn sync_mark_prefs_dirty(&mut self) {
        if !self.sync.track_manifest {
            return;
        }
        self.sync.prefs_dirty_pending = true;
        let hash = canonical_prefs_hash(&self.prefs);
        self.sync.manifest.mark_prefs_dirty(&hash);
        self.sync_reconcile_folders();
        self.sync.last_dirty_at_ms.get_or_insert_with(now_unix_ms);
        self.mark_offline_edits_if_disconnected();
        self.sync_persist_manifest();
    }

    pub(crate) fn sync_before_remove_slot(&mut self, index: usize) {
        if !self.sync.track_manifest {
            return;
        }
        self.ensure_sync_vec_lens(self.notebooks.len());
        if index < self.sync.slot_ids.len() {
            let id = self.sync.slot_ids.remove(index);
            if !id.is_empty() {
                self.sync.manifest.record_local_delete(&id, now_unix_ms());
                self.sync.last_dirty_at_ms.get_or_insert_with(now_unix_ms);
                self.mark_offline_edits_if_disconnected();
            }
        }
        if index < self.sync.slot_dirty_hash.len() {
            self.sync.slot_dirty_hash.remove(index);
        }
        if index < self.sync.slot_placeholder_blank.len() {
            self.sync.slot_placeholder_blank.remove(index);
        }
        self.sync_persist_manifest();
    }

    pub(crate) fn mark_offline_edits_if_disconnected(&mut self) {
        let offline_now = self.sync.network_probed_offline
            || matches!(self.sync.connection, SyncConnectionState::Offline);
        let synced_before = self.sync.manifest.last_sync_at_ms.is_some()
            || self.sync.manifest.last_server_cursor > 0;
        if offline_now
            && self.is_viewing_cloud()
            && synced_before
            && !self.sync.manifest.offline_edits_pending
        {
            self.sync.manifest.offline_edits_pending = true;
            self.sync_persist_manifest();
        }
    }

    pub(crate) fn sync_restore_notebook_slot(&mut self, index: usize, notebook_id: &str) {
        if !self.sync.track_manifest {
            return;
        }
        let clean_id = sanitize_notebook_id(notebook_id);
        let id = if clean_id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            clean_id
        };
        let restored_base_version = self
            .sync
            .manifest
            .tombstones
            .iter()
            .find(|t| t.notebook_id == id)
            .map_or(0, |t| t.base_version);
        self.sync.manifest.clear_tombstone(&id);
        self.ensure_sync_vec_lens(self.notebooks.len());
        if index < self.sync.slot_ids.len() {
            self.sync.slot_ids[index] = id.clone();
            self.sync.slot_dirty_hash[index] = true;
            self.sync.slot_placeholder_blank[index] = false;
        }
        if let Some(doc) = self.notebooks.get(index) {
            let hash = canonical_document_hash(doc);
            self.sync.manifest.mark_notebook_dirty(&id, &hash);
            if restored_base_version > 0
                && let Some(meta) = self
                    .sync
                    .manifest
                    .notebooks
                    .iter_mut()
                    .find(|m| m.notebook_id == id)
            {
                meta.server_version = restored_base_version;
                meta.last_synced_hash.clear();
            }
        }
        self.sync.last_dirty_at_ms.get_or_insert_with(now_unix_ms);
        self.sync_persist_manifest();
    }

    pub(crate) fn sync_reconcile_slots(&mut self) {
        if !self.sync.track_manifest {
            return;
        }
        let count = self.notebooks.len();
        let mut changed = self.sync.slot_ids.len() != count;
        self.ensure_sync_vec_lens(count);

        for idx in 0..count {
            if self.sync.slot_placeholder_blank[idx] {
                continue;
            }
            let file_name = self.library.slot_files.get(idx).and_then(slot_file_name);
            let embedded_id = sanitize_notebook_id(&self.notebooks[idx].id);
            if self.sync.slot_ids[idx].is_empty() || !embedded_id.is_empty() {
                let id = if !embedded_id.is_empty() {
                    embedded_id
                } else {
                    let resolved = file_name
                        .as_deref()
                        .and_then(|name| self.sync.manifest.id_for_local_file(name))
                        .map(str::to_string)
                        .unwrap_or_else(|| Uuid::new_v4().to_string());
                    let mut updated_doc = (*self.notebooks[idx]).clone();
                    updated_doc.id = resolved.clone();
                    let arc = Arc::new(updated_doc);
                    self.notebooks[idx] = Arc::clone(&arc);
                    if idx == self.active_notebook {
                        self.doc = arc;
                    }
                    self.library.mark_dirty(idx, count);
                    resolved
                };
                if self.sync.slot_ids[idx] != id {
                    self.sync.slot_ids[idx] = id.clone();
                    changed = true;
                }
                if self.sync.manifest.meta(&id).is_none() {
                    let hash = canonical_document_hash(&self.notebooks[idx]);
                    self.sync.manifest.mark_notebook_dirty(&id, &hash);
                    self.sync.last_dirty_at_ms.get_or_insert_with(now_unix_ms);
                    changed = true;
                }
                if let Some(name) = file_name.as_deref() {
                    let bound = self
                        .sync
                        .manifest
                        .meta(&id)
                        .and_then(|m| m.local_file.as_deref());
                    if bound != Some(name) {
                        self.sync.manifest.bind_local_file(&id, name);
                        changed = true;
                    }
                }
            } else if let Some(name) = file_name.as_deref() {
                let id = &self.sync.slot_ids[idx];
                let bound = self
                    .sync
                    .manifest
                    .meta(id)
                    .and_then(|m| m.local_file.as_deref());
                if bound != Some(name) {
                    self.sync.manifest.bind_local_file(id, name);
                    changed = true;
                }
            }
        }

        if changed {
            self.sync_persist_manifest();
        }
    }

    fn sync_init_from_disk(&mut self, is_placeholder: bool, owner: OwnerId) {
        let Some(ref lib_dir) = self.library.dir else {
            return;
        };
        self.sync.manifest = load_manifest(lib_dir);
        self.sync.manifest.owner_id = Some(owner);
        let count = self.notebooks.len();
        self.sync.slot_ids = vec![String::new(); count];
        self.sync.slot_dirty_hash = vec![false; count];
        self.sync.slot_placeholder_blank = vec![is_placeholder && count == 1; count];
        self.sync.prefs_dirty_pending = false;
        self.sync.last_dirty_at_ms = None;

        if !is_placeholder {
            self.sync_reconcile_slots();
            self.sync_reconcile_folders();
            for idx in 0..count {
                let id = self.sync.slot_ids[idx].clone();
                let cur_hash = canonical_document_hash(&self.notebooks[idx]);
                let is_diff = self
                    .sync
                    .manifest
                    .meta(&id)
                    .is_none_or(|m| m.server_version == 0 || m.last_synced_hash != cur_hash);
                if is_diff {
                    self.sync.manifest.mark_notebook_dirty(&id, &cur_hash);
                }
            }
            let prefs_hash = canonical_prefs_hash(&self.prefs);
            if self.sync.manifest.prefs_dirty
                || (self.sync.manifest.prefs_version > 0
                    && self.sync.manifest.prefs_hash != prefs_hash)
            {
                self.sync.manifest.mark_prefs_dirty(&prefs_hash);
            }
        }
        if self.sync.manifest.pending_changes_count() > 0 {
            self.sync.last_dirty_at_ms = Some(now_unix_ms());
        }
        self.sync_persist_manifest();
    }

    fn sync_reconcile_folders(&mut self) {
        if !self.sync.track_manifest {
            return;
        }
        let now = now_unix_ms();
        let owner = self.sync.manifest.owner_id.clone();
        let is_cloud = owner.as_ref().is_some_and(OwnerId::is_cloud);
        let current_paths = sanitize_library_folders(&self.prefs.library_folders);
        let mut changed = false;

        for path in &current_paths {
            let color_hex = self.prefs.folder_color(path).map(Color::to_hex);
            if let Some(existing) = self
                .sync
                .manifest
                .folders
                .iter_mut()
                .find(|f| f.path.eq_ignore_ascii_case(path))
            {
                if existing.path != *path
                    || existing.color_hex != color_hex
                    || existing.deleted_at_ms.is_some()
                {
                    existing.path = path.clone();
                    existing.color_hex = color_hex;
                    existing.deleted_at_ms = None;
                    existing.updated_at_ms = now.max(existing.updated_at_ms);
                    existing.cloud_dirty = is_cloud;
                    existing.status = if is_cloud {
                        EntitySyncStatus::PendingPush
                    } else {
                        EntitySyncStatus::LocalOnly
                    };
                    changed = true;
                }
            } else {
                self.sync.manifest.folders.push(FolderSyncMeta {
                    folder_id: Uuid::new_v4().to_string(),
                    owner_id: owner.clone(),
                    path: path.clone(),
                    color_hex,
                    status: if is_cloud {
                        EntitySyncStatus::PendingPush
                    } else {
                        EntitySyncStatus::LocalOnly
                    },
                    server_version: 0,
                    cloud_dirty: is_cloud,
                    updated_at_ms: now,
                    deleted_at_ms: None,
                });
                changed = true;
            }
        }

        // Track tombstones for previously synced folders that were removed from prefs.
        let mut keep = Vec::with_capacity(self.sync.manifest.folders.len());
        for mut folder in std::mem::take(&mut self.sync.manifest.folders) {
            let still_present = current_paths
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&folder.path));
            if still_present {
                keep.push(folder);
            } else if folder.server_version > 0 && is_cloud {
                if folder.deleted_at_ms.is_none() {
                    folder.deleted_at_ms = Some(now);
                    folder.updated_at_ms = now.max(folder.updated_at_ms);
                    folder.cloud_dirty = true;
                    folder.status = EntitySyncStatus::PendingPush;
                    changed = true;
                }
                keep.push(folder);
            } else {
                changed = true;
            }
        }
        self.sync.manifest.folders = keep;
        if changed {
            self.sync_persist_manifest();
        }
    }

    fn ack_accepted_folders(&mut self, accepted: &[drawcraft_sync_protocol::AcceptedFolderSync]) {
        for acc in accepted {
            if acc.deleted {
                self.sync
                    .manifest
                    .folders
                    .retain(|f| f.folder_id != acc.folder_id);
            } else if let Some(entry) = self
                .sync
                .manifest
                .folders
                .iter_mut()
                .find(|f| f.folder_id == acc.folder_id)
            {
                entry.server_version = acc.new_version;
                entry.cloud_dirty = false;
                entry.status = EntitySyncStatus::Synced;
            }
        }
    }

    fn apply_remote_folders(&mut self, remote_folders: &[RemoteFolderRecord]) -> bool {
        if remote_folders.is_empty() {
            return false;
        }
        let mut changed = false;
        for rf in remote_folders {
            let existing_pos =
                self.sync.manifest.folders.iter().position(|f| {
                    f.folder_id == rf.folder_id || f.path.eq_ignore_ascii_case(&rf.path)
                });
            if rf.deleted {
                if let Some(pos) = existing_pos {
                    let removed = self.sync.manifest.folders.remove(pos);
                    self.prefs
                        .library_folders
                        .retain(|p| !p.eq_ignore_ascii_case(&removed.path));
                    self.prefs.set_folder_color(&removed.path, None);
                    changed = true;
                }
                continue;
            }
            let clean_path = sanitize_folder_path(&rf.path);
            if clean_path.is_empty() {
                continue;
            }
            let parsed_color = rf.color_hex.as_deref().and_then(Color::from_hex);
            let is_local_newer = existing_pos.is_some_and(|pos| {
                let cur = &self.sync.manifest.folders[pos];
                cur.cloud_dirty && cur.updated_at_ms > rf.updated_at_ms
            });
            if is_local_newer {
                continue;
            }
            if let Some(pos) = existing_pos {
                let old_path = self.sync.manifest.folders[pos].path.clone();
                if !old_path.eq_ignore_ascii_case(&clean_path) {
                    self.prefs
                        .library_folders
                        .retain(|p| !p.eq_ignore_ascii_case(&old_path));
                }
                let entry = &mut self.sync.manifest.folders[pos];
                entry.folder_id = rf.folder_id.clone();
                entry.path = clean_path.clone();
                entry.color_hex = rf.color_hex.clone();
                entry.server_version = rf.version;
                entry.cloud_dirty = false;
                entry.deleted_at_ms = None;
                entry.updated_at_ms = rf.updated_at_ms;
                entry.status = EntitySyncStatus::Synced;
            } else {
                self.sync.manifest.folders.push(FolderSyncMeta {
                    folder_id: rf.folder_id.clone(),
                    owner_id: self.sync.manifest.owner_id.clone(),
                    path: clean_path.clone(),
                    color_hex: rf.color_hex.clone(),
                    status: EntitySyncStatus::Synced,
                    server_version: rf.version,
                    cloud_dirty: false,
                    updated_at_ms: rf.updated_at_ms,
                    deleted_at_ms: None,
                });
            }
            self.prefs.library_folders.push(clean_path.clone());
            self.prefs.set_folder_color(&clean_path, parsed_color);
            changed = true;
        }
        if changed {
            self.prefs.library_folders = sanitize_library_folders(&self.prefs.library_folders);
            self.prefs = self.prefs.sanitized();
            let _ = self.save_prefs();
        }
        changed
    }

    fn apply_remote_prefs_preserving_folders(&mut self, remote_prefs: &UserStudioConfig) {
        let local_folders = self.prefs.library_folders.clone();
        let local_folder_colors = self.prefs.library_folder_colors.clone();
        let mut merged = remote_prefs.clone().sanitized();
        if !self.sync.manifest.folders.is_empty() {
            merged.library_folders = local_folders;
            merged.library_folder_colors = local_folder_colors;
        }
        self.prefs = merged.sanitized();
        self.apply_pen_size_preferences();
    }

    fn compute_pending_changes_fast(&self) -> usize {
        let mut count = self.sync.manifest.pending_changes_count();
        for (idx, dirty) in self.sync.slot_dirty_hash.iter().enumerate() {
            if !*dirty
                || self
                    .sync
                    .slot_placeholder_blank
                    .get(idx)
                    .copied()
                    .unwrap_or(false)
            {
                continue;
            }
            let already_counted = self
                .sync
                .slot_ids
                .get(idx)
                .and_then(|id| self.sync.manifest.meta(id))
                .is_some_and(|m| m.cloud_dirty || m.server_version == 0);
            if !already_counted {
                count += 1;
            }
        }
        if self.sync.prefs_dirty_pending && !self.sync.manifest.prefs_dirty {
            count += 1;
        }
        count
    }

    fn ensure_sync_vec_lens(&mut self, count: usize) {
        self.sync.slot_ids.resize_with(count, String::new);
        self.sync.slot_dirty_hash.resize(count, false);
        self.sync.slot_placeholder_blank.resize(count, false);
    }

    fn sync_persist_global_state(&self) {
        if let Some(ref root) = self.sync.desktop_root
            && let Err(err) = save_global_state(root, &self.sync.global)
        {
            log::warn!("Failed saving global sync state: {err}");
        }
    }

    pub(crate) fn sync_persist_manifest(&self) {
        if !self.sync.track_manifest {
            return;
        }
        if let Some(ref dir) = self.library.dir
            && let Err(err) = save_manifest(dir, &self.sync.manifest)
        {
            log::warn!("Failed saving sync manifest: {err}");
        }
    }

    fn ack_accepted_uploads(
        &mut self,
        accepted: &[drawcraft_sync_protocol::AcceptedNotebookSync],
        synced_at_ms: i64,
    ) {
        for acc in accepted {
            let cur_hash = self
                .find_slot_by_id(&acc.notebook_id)
                .map(|idx| canonical_document_hash(&self.notebooks[idx]))
                .unwrap_or_else(|| acc.content_hash.clone());
            self.sync.manifest.mark_notebook_synced(
                &acc.notebook_id,
                acc.new_version,
                acc.content_hash.clone(),
                &cur_hash,
                synced_at_ms,
            );
            if let Some(idx) = self.find_slot_by_id(&acc.notebook_id)
                && cur_hash == acc.content_hash
            {
                self.sync.slot_dirty_hash[idx] = false;
            }
        }
    }

    fn apply_remote_upsert(
        &mut self,
        update: &RemoteNotebookUpdate,
        synced_at_ms: i64,
        notice_titles: &mut Vec<String>,
    ) -> bool {
        let Some(mut doc) = update.document.clone() else {
            return false;
        };
        doc.id = update.notebook_id.clone();
        doc.sync_active_to_pages();
        doc.load_active_from_pages();

        if let Some(idx) = self.find_slot_by_id(&update.notebook_id) {
            let cur_hash = canonical_document_hash(&self.notebooks[idx]);
            let local_changed = self.sync.slot_dirty_hash[idx]
                || self
                    .sync
                    .manifest
                    .meta(&update.notebook_id)
                    .is_some_and(|m| m.cloud_dirty && cur_hash != m.last_synced_hash);

            if local_changed && cur_hash != update.content_hash {
                let local_body = canonical_document_body_hash(&self.notebooks[idx]);
                let remote_body = canonical_document_body_hash(&doc);
                if local_body == remote_body {
                    // Metadata-only divergence: merge via Last-Write-Wins without creating duplicates.
                    let local_doc = &self.notebooks[idx];
                    if local_doc.meta_updated_at_ms > doc.meta_updated_at_ms {
                        doc.title = local_doc.title.clone();
                        doc.cover = local_doc.cover;
                        doc.folder_path = local_doc.folder_path.clone();
                        doc.favorite = local_doc.favorite;
                        doc.tags = local_doc.tags.clone();
                        doc.meta_updated_at_ms = local_doc.meta_updated_at_ms;
                    }
                    doc.sync_active_to_pages();
                    let merged_hash = canonical_document_hash(&doc);
                    self.notebooks[idx] = Arc::new(doc);
                    self.library.mark_dirty(idx, self.notebooks.len());
                    self.sync.slot_dirty_hash[idx] = merged_hash != update.content_hash;
                    self.sync.slot_placeholder_blank[idx] = false;
                    if idx == self.active_notebook {
                        self.doc = Arc::clone(&self.notebooks[idx]);
                        self.history.clear();
                    }
                    self.sync.manifest.mark_notebook_synced(
                        &update.notebook_id,
                        update.version,
                        update.content_hash.clone(),
                        &merged_hash,
                        synced_at_ms,
                    );
                    return true;
                }

                let mut local_copy = (*self.notebooks[idx]).clone();
                let copy_id = Uuid::new_v4().to_string();
                let copy_title = drawcraft_sync::make_conflict_title(&local_copy.title);
                local_copy.id = copy_id.clone();
                local_copy.title = copy_title.clone();
                let copy_hash = canonical_document_hash(&local_copy);
                notice_titles.push(copy_title);
                self.insert_conflict_copy_slot(
                    &update.notebook_id,
                    &copy_id,
                    local_copy,
                    &copy_hash,
                );
            }

            self.notebooks[idx] = Arc::new(doc);
            self.library.mark_dirty(idx, self.notebooks.len());
            self.sync.slot_dirty_hash[idx] = false;
            self.sync.slot_placeholder_blank[idx] = false;
            if idx == self.active_notebook {
                self.doc = Arc::clone(&self.notebooks[idx]);
                self.history.clear();
            }
            self.sync.manifest.mark_notebook_synced(
                &update.notebook_id,
                update.version,
                update.content_hash.clone(),
                &update.content_hash,
                synced_at_ms,
            );
            return true;
        }

        // Check for an untouched blank login placeholder we can replace seamlessly.
        if let Some(p_idx) = self.sync.slot_placeholder_blank.iter().position(|&b| b) {
            self.notebooks[p_idx] = Arc::new(doc);
            if self
                .library
                .slot_files
                .get(p_idx)
                .is_some_and(Option::is_none)
                && let Some(dir) = self.library.dir.clone()
            {
                self.library.slot_files[p_idx] = Some(self.library.alloc_file_path(&dir));
            }
            self.library.mark_dirty(p_idx, self.notebooks.len());
            self.sync.slot_ids[p_idx] = update.notebook_id.clone();
            self.sync.slot_dirty_hash[p_idx] = false;
            self.sync.slot_placeholder_blank[p_idx] = false;
            if p_idx == self.active_notebook {
                self.doc = Arc::clone(&self.notebooks[p_idx]);
                self.history.clear();
            }
            self.sync.manifest.mark_notebook_synced(
                &update.notebook_id,
                update.version,
                update.content_hash.clone(),
                &update.content_hash,
                synced_at_ms,
            );
            if let Some(file_name) = self.library.slot_files.get(p_idx).and_then(slot_file_name) {
                self.sync
                    .manifest
                    .bind_local_file(&update.notebook_id, &file_name);
            }
            return true;
        }

        if self.notebooks.len() >= MAX_NOTEBOOKS {
            log::warn!(
                "Skipping remote notebook `{}`: reached MAX_NOTEBOOKS ({MAX_NOTEBOOKS})",
                update.notebook_id
            );
            return false;
        }

        self.notebooks.push(Arc::new(doc));
        let path = self
            .library
            .dir
            .clone()
            .map(|dir| self.library.alloc_file_path(&dir));
        let file_name = slot_file_name(&path);
        self.library.push_slot(path, true, self.notebooks.len());
        self.sync.slot_ids.push(update.notebook_id.clone());
        self.sync.slot_dirty_hash.push(false);
        self.sync.slot_placeholder_blank.push(false);
        self.sync.manifest.mark_notebook_synced(
            &update.notebook_id,
            update.version,
            update.content_hash.clone(),
            &update.content_hash,
            synced_at_ms,
        );
        if let Some(name) = file_name {
            self.sync
                .manifest
                .bind_local_file(&update.notebook_id, &name);
        }
        true
    }

    fn apply_remote_delete(&mut self, notebook_id: &str) -> bool {
        self.sync.manifest.clear_tombstone(notebook_id);
        self.sync
            .manifest
            .notebooks
            .retain(|m| m.notebook_id != notebook_id);

        let Some(idx) = self.find_slot_by_id(notebook_id) else {
            return false;
        };

        if self.notebooks.len() <= 1 {
            let mut blank = Document::default();
            blank.sync_active_to_pages();
            self.notebooks[0] = Arc::new(blank);
            self.doc = Arc::clone(&self.notebooks[0]);
            self.active_notebook = 0;
            self.history.clear();
            if let Some(slot) = self.library.slot_files.get_mut(0)
                && let Some(old_path) = slot.take()
            {
                let _ = fs::remove_file(old_path);
            }
            self.library.slot_dirty[0] = false;
            self.sync.slot_ids[0].clear();
            self.sync.slot_dirty_hash[0] = false;
            self.sync.slot_placeholder_blank[0] = true;
            return true;
        }

        self.notebooks.remove(idx);
        self.library.remove_slot(idx);
        self.sync.slot_ids.remove(idx);
        self.sync.slot_dirty_hash.remove(idx);
        self.sync.slot_placeholder_blank.remove(idx);

        if idx < self.active_notebook {
            self.active_notebook = self.active_notebook.saturating_sub(1);
        } else if self.active_notebook >= self.notebooks.len() {
            self.active_notebook = self.notebooks.len().saturating_sub(1);
        }
        self.doc = Arc::clone(&self.notebooks[self.active_notebook]);
        self.history.clear();
        true
    }

    fn insert_conflict_copy_slot(
        &mut self,
        original_id: &str,
        copy_id: &str,
        mut doc: Document,
        copy_hash: &str,
    ) {
        if self.notebooks.len() >= MAX_NOTEBOOKS {
            log::warn!("Cannot add conflict copy `{copy_id}`: MAX_NOTEBOOKS reached");
            return;
        }
        doc.id = copy_id.to_string();
        doc.sync_active_to_pages();
        self.notebooks.push(Arc::new(doc));
        let path = self
            .library
            .dir
            .clone()
            .map(|dir| self.library.alloc_file_path(&dir));
        let file_name = slot_file_name(&path);
        self.library.push_slot(path, true, self.notebooks.len());
        self.sync.slot_ids.push(copy_id.to_string());
        self.sync.slot_dirty_hash.push(false);
        self.sync.slot_placeholder_blank.push(false);
        self.sync
            .manifest
            .mark_notebook_conflict(copy_id, original_id, copy_hash);
        if let Some(name) = file_name {
            self.sync.manifest.bind_local_file(copy_id, &name);
        }
        self.sync.last_dirty_at_ms = Some(now_unix_ms());
    }

    pub(crate) fn find_slot_by_id(&self, notebook_id: &str) -> Option<usize> {
        self.sync.slot_ids.iter().position(|id| id == notebook_id)
    }
}

struct CapturedProfileData {
    docs: Vec<Document>,
    trash: Vec<CachedTrashEntry>,
    prefs: UserStudioConfig,
}

/// Renames a local notebook title to indicate its origin account (e.g. `Math - Bruno`).
pub fn rename_migrated_notebook_title(raw_title: &str, raw_nickname: &str) -> String {
    let base = sanitize_notebook_title(raw_title);
    let nick = sanitize_profile_nickname(raw_nickname);
    let suffix = format!(" - {nick}");
    if base.ends_with(&suffix) {
        return base;
    }
    let max_total = 120usize;
    let suffix_chars = suffix.chars().count();
    let max_base_chars = max_total.saturating_sub(suffix_chars);
    let truncated_base: String = base.chars().take(max_base_chars).collect();
    sanitize_notebook_title(&format!("{truncated_base}{suffix}"))
}

/// Merges folders, colors, custom templates, elements, and style presets cleanly into `target`.
pub(crate) fn merge_studio_prefs(
    target: &mut UserStudioConfig,
    incoming: UserStudioConfig,
    overwrite_appearance: bool,
) {
    let mut combined_folders = target.library_folders.clone();
    combined_folders.extend(incoming.library_folders);
    target.library_folders = sanitize_library_folders(&combined_folders);

    for (k, v) in incoming.library_folder_colors {
        if overwrite_appearance {
            target.library_folder_colors.insert(k, v);
        } else {
            target.library_folder_colors.entry(k).or_insert(v);
        }
    }
    for (k, v) in incoming.library_tag_colors {
        if overwrite_appearance {
            target.library_tag_colors.insert(k, v);
        } else {
            target.library_tag_colors.entry(k).or_insert(v);
        }
    }
    for tmpl in incoming.custom_templates {
        if !target.custom_templates.iter().any(|t| t.id == tmpl.id) {
            target.custom_templates.push(tmpl);
        }
    }
    for elem in incoming.saved_elements {
        if !target.saved_elements.iter().any(|e| e.id == elem.id) {
            target.saved_elements.push(elem);
        }
    }
    for fav in incoming.style_favorites {
        if !target.style_favorites.iter().any(|f| f.id == fav.id) {
            target.style_favorites.push(fav);
        }
    }
    for preset in incoming.palette_presets {
        if !target.palette_presets.iter().any(|p| p.id == preset.id) {
            target.palette_presets.push(preset);
        }
    }
    if overwrite_appearance {
        target.theme = incoming.theme;
        target.auto_contrast_with_theme = incoming.auto_contrast_with_theme;
        target.ui_scale = incoming.ui_scale;
        target.pen_sizes = incoming.pen_sizes;
        target.ink_palette = incoming.ink_palette;
        target.highlighter_palette = incoming.highlighter_palette;
        target.wildcard_ink = incoming.wildcard_ink;
        target.wildcard_highlighter = incoming.wildcard_highlighter;
        target.active_palette_id = incoming.active_palette_id;
        target.stylus_buttons = incoming.stylus_buttons;
        target.library_view_mode = incoming.library_view_mode;
        target.library_sort_by = incoming.library_sort_by;
        target.library_sort_ascending = incoming.library_sort_ascending;
    }
    *target = target.sanitized();
}

fn ensure_default_profile_in_global(global: &mut GlobalSyncState) -> String {
    if let Some(active) = global.active_local_profile() {
        let id = active.local_profile_id.clone();
        if global.active_local_profile_id != id {
            global.active_local_profile_id = id.clone();
        }
        return id;
    }
    let now = now_unix_ms();
    let id = Uuid::new_v4().to_string();
    let owner_cloud_user_id = global.active_cloud_user_id().map(str::to_string);
    global.local_profiles.push(LocalProfile {
        local_profile_id: id.clone(),
        nickname: "Personal".to_string(),
        created_at_ms: now,
        last_used_at_ms: now,
        owner_cloud_user_id,
    });
    global.active_local_profile_id = id.clone();
    id
}

fn recover_existing_desktop_profiles(root_dir: &Path, global: &mut GlobalSyncState) -> bool {
    let profiles_root = root_dir.join(drawcraft_sync::PROFILES_DIR);
    if !profiles_root.is_dir() {
        return false;
    }
    let Ok(entries) = fs::read_dir(&profiles_root) else {
        return false;
    };
    let mut found: Vec<(usize, String)> = Vec::new();
    for entry in entries.flatten().take(64) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(raw_id) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if raw_id.is_empty() || sanitize_account_dir_segment(raw_id) != raw_id {
            continue;
        }
        let ncraft_count = count_ncraft_files_in_dir(&path);
        let has_data = ncraft_count > 0
            || path.join(LIBRARY_MARKER_FILE).exists()
            || path.join(STUDIO_PREFS_FILE).exists();
        if has_data {
            found.push((ncraft_count, raw_id.to_string()));
        }
    }
    if found.is_empty() {
        return false;
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let now = now_unix_ms();
    global.local_profiles = found
        .into_iter()
        .enumerate()
        .map(|(idx, (_, id))| LocalProfile {
            local_profile_id: id,
            nickname: if idx == 0 {
                "Personal".to_string()
            } else {
                format!("Recovered {}", idx + 1)
            },
            created_at_ms: now,
            last_used_at_ms: now,
            owner_cloud_user_id: None,
        })
        .collect();
    if let Some(first) = global.local_profiles.first() {
        global.active_local_profile_id = first.local_profile_id.clone();
    }
    global.needs_initial_nickname = false;
    true
}

fn ensure_desktop_default_profile(root_dir: &Path, global: &mut GlobalSyncState) -> bool {
    if !global.accessible_local_profiles().is_empty() {
        let before = global.active_local_profile_id.clone();
        let resolved = ensure_default_profile_in_global(global);
        return resolved != before;
    }
    if global.local_profiles.is_empty() && recover_existing_desktop_profiles(root_dir, global) {
        return true;
    }
    let pid = ensure_default_profile_in_global(global);
    let target_dir = profile_library_dir(root_dir, &pid);
    if !target_dir.exists() {
        migrate_legacy_root_library(root_dir, &target_dir);
    }
    true
}

fn has_legacy_root_library(root_dir: &Path) -> bool {
    root_dir.join(LIBRARY_MARKER_FILE).exists()
        || root_dir.join(STUDIO_PREFS_FILE).exists()
        || root_dir.join(TRASH_SUBDIR).is_dir()
        || fs::read_dir(root_dir).ok().is_some_and(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("dcraft")
                            || ext.eq_ignore_ascii_case("scraft")
                            || ext.eq_ignore_ascii_case("pcraft")
                            || ext.eq_ignore_ascii_case("ncraft")
                    })
            })
        })
}

fn migrate_legacy_root_library(root_dir: &Path, target_dir: &Path) {
    if !has_legacy_root_library(root_dir) {
        return;
    }
    let _ = fs::create_dir_all(target_dir);
    for fixed in [
        LIBRARY_MARKER_FILE,
        STUDIO_PREFS_FILE,
        drawcraft_sync::SYNC_MANIFEST_FILE,
        TRASH_SUBDIR,
    ] {
        let src = root_dir.join(fixed);
        if src.exists() {
            let _ = fs::rename(&src, target_dir.join(fixed));
        }
    }
    if let Ok(entries) = fs::read_dir(root_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("dcraft")
                            || ext.eq_ignore_ascii_case("scraft")
                            || ext.eq_ignore_ascii_case("pcraft")
                            || ext.eq_ignore_ascii_case("ncraft")
                    })
                && let Some(name) = path.file_name()
            {
                let _ = fs::rename(&path, target_dir.join(name));
            }
        }
    }
}

fn migrate_web_cache_if_needed(cache: &mut WebWorkspaceCache) {
    if cache.local_profiles.is_empty()
        && let Some(guest_bundle) = cache.guest.take()
    {
        let pid = ensure_default_profile_in_global(&mut cache.global);
        cache.local_profiles.insert(pid, guest_bundle);
    }
    cache.version = WEB_CACHE_VERSION;
}

fn sanitize_global_state(global: &mut GlobalSyncState) {
    global.server_url =
        normalize_server_url(&global.server_url).unwrap_or_else(|_| DEFAULT_SERVER_URL.to_string());
    global.device_name = sanitize_device_name(&global.device_name);
    global.client_id = sanitize_client_id(&global.client_id);
    if global.client_id.is_empty() {
        global.client_id = Uuid::new_v4().to_string();
    }
    if let Some(session) = global.session.as_mut() {
        if let Ok(norm) = normalize_server_url(&session.server_url) {
            session.server_url = norm;
        }
        session.device_name = sanitize_device_name(&session.device_name);
    }
    for prof in &mut global.local_profiles {
        prof.nickname = sanitize_profile_nickname(&prof.nickname);
    }
}

fn starter_web_bundle() -> CachedLibraryBundle {
    CachedLibraryBundle::default()
}

fn placeholder_web_bundle() -> CachedLibraryBundle {
    let mut blank = Document::default();
    blank.sync_active_to_pages();
    CachedLibraryBundle {
        notebooks: vec![blank],
        slot_placeholder_blank: vec![true],
        ..Default::default()
    }
}

fn restore_web_bundle(bundle: &CachedLibraryBundle) -> Session {
    let docs: Vec<Arc<Document>> = bundle
        .notebooks
        .iter()
        .take(MAX_NOTEBOOKS)
        .cloned()
        .map(|mut d| {
            d.id = sanitize_notebook_id(&d.id);
            d.title = sanitize_notebook_title(&d.title);
            d.sync_active_to_pages();
            d.load_active_from_pages();
            Arc::new(d)
        })
        .collect();

    let mut session = Session {
        prefs: bundle.prefs.clone().sanitized(),
        ..Default::default()
    };
    session.apply_pen_size_preferences();
    if docs.is_empty() {
        session.notebooks.clear();
        let mut blank = Document::default();
        blank.sync_active_to_pages();
        session.doc = Arc::new(blank);
        session.active_notebook = 0;
    } else {
        let active = bundle.active_notebook.min(docs.len().saturating_sub(1));
        session.notebooks = docs;
        session.active_notebook = active;
        session.doc = Arc::clone(&session.notebooks[active]);
    }

    let count = session.notebooks.len();
    session.library.sync_len(count);
    session.trashed_notebooks = bundle
        .trash
        .iter()
        .map(|entry| {
            let mut d = entry.doc.clone();
            d.id = sanitize_notebook_id(&d.id);
            d.title = sanitize_notebook_title(&d.title);
            d.sync_active_to_pages();
            d.load_active_from_pages();
            TrashedNotebook {
                doc: Arc::new(d),
                deleted_at_ms: entry.deleted_at_ms,
                path: None,
            }
        })
        .collect();
    session.cleanup_expired_trash();

    session.sync.manifest = bundle.manifest.clone();
    session.sync.slot_ids = bundle.slot_ids.clone();
    session.sync.slot_dirty_hash = bundle.slot_dirty_hash.clone();
    session.sync.slot_placeholder_blank = bundle.slot_placeholder_blank.clone();
    session.sync.prefs_dirty_pending = bundle.prefs_dirty_pending;
    session.sync.last_dirty_at_ms = bundle.last_dirty_at_ms;
    session.ensure_sync_vec_lens(count);
    session
}

fn merge_captured_into_session(
    session: &mut Session,
    captured: CapturedProfileData,
    owner: OwnerId,
    disk_dir: Option<&Path>,
) -> bool {
    session.sync.manifest.owner_id = Some(owner);
    if !captured.docs.is_empty() || !captured.prefs.library_folders.is_empty() {
        merge_studio_prefs(&mut session.prefs, captured.prefs, false);
        session.sync_mark_prefs_dirty();
    }

    let is_placeholder = session
        .sync
        .slot_placeholder_blank
        .first()
        .copied()
        .unwrap_or(false)
        || session.notebooks.is_empty();
    let mut placeholder_consumed = !is_placeholder && !session.notebooks.is_empty();

    for mut doc in captured.docs {
        doc.sync_active_to_pages();
        if doc.id.is_empty() {
            doc.id = Uuid::new_v4().to_string();
        }
        let doc_id = doc.id.clone();
        let already_exists = session
            .notebooks
            .iter()
            .any(|existing| existing.id == doc_id);
        if placeholder_consumed && already_exists {
            continue;
        }
        let doc_hash = canonical_document_hash(&doc);
        if !placeholder_consumed && !session.notebooks.is_empty() {
            session.notebooks[0] = Arc::new(doc);
            session.doc = Arc::clone(&session.notebooks[0]);
            if let Some(dir) = disk_dir {
                session.library.slot_files[0] = Some(session.library.alloc_file_path(dir));
            }
            session.library.mark_dirty(0, session.notebooks.len());
            session.ensure_sync_vec_lens(session.notebooks.len());
            session.sync.slot_ids[0] = doc_id.clone();
            session.sync.slot_dirty_hash[0] = true;
            session.sync.slot_placeholder_blank[0] = false;
            placeholder_consumed = true;
        } else if session.notebooks.len() < MAX_NOTEBOOKS {
            session.notebooks.push(Arc::new(doc));
            let path = disk_dir.map(|d| session.library.alloc_file_path(d));
            session
                .library
                .push_slot(path, true, session.notebooks.len());
            session.sync.slot_ids.push(doc_id.clone());
            session.sync.slot_dirty_hash.push(true);
            session.sync.slot_placeholder_blank.push(false);
        }
        session
            .sync
            .manifest
            .mark_notebook_dirty(&doc_id, &doc_hash);
    }

    if let Some(dir) = disk_dir {
        let _ = session.save_library();
        let _ = session.save_prefs();
        let _ = fs::write(dir.join(LIBRARY_MARKER_FILE), b"1");
    }
    !placeholder_consumed
}

fn slot_file_name(opt_path: &Option<PathBuf>) -> Option<String> {
    opt_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
}

fn open_or_merge_account_dir(
    acct_dir: &Path,
    owner: OwnerId,
    captured: Option<CapturedProfileData>,
) -> (Session, bool) {
    let _ = fs::create_dir_all(acct_dir);
    let (mut s, is_placeholder) = Session::open_library_mode(
        acct_dir.to_path_buf(),
        LibraryInitMode::BlankPlaceholderOnly,
    );
    s.sync.track_manifest = true;
    s.sync.manifest = load_manifest(acct_dir);
    s.sync.manifest.owner_id = Some(owner.clone());
    s.sync.slot_ids = vec![String::new(); s.notebooks.len()];
    s.sync.slot_dirty_hash = vec![false; s.notebooks.len()];
    s.sync.slot_placeholder_blank =
        vec![is_placeholder && s.notebooks.len() == 1; s.notebooks.len()];

    if let Some(source) = captured {
        if !is_placeholder {
            s.sync_reconcile_slots();
        } else if !source.docs.is_empty() {
            s.prefs = source.prefs.clone();
            s.apply_pen_size_preferences();
        }
        let still_placeholder = merge_captured_into_session(&mut s, source, owner, Some(acct_dir));
        let _ = save_manifest(acct_dir, &s.sync.manifest);
        return (s, still_placeholder);
    }

    (s, is_placeholder)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{AuthWorkspaceMode, Session};
    use drawcraft_doc::{Document, NotebookCover, PenSettings, StrokeSample};
    use drawcraft_sync::{
        ActiveAccountSession, EntitySyncStatus, RemoteNotebookUpdate, SyncCycleOutcome,
    };
    use drawcraft_sync_protocol::{
        UserProfile, canonical_document_body_hash, canonical_document_hash,
        canonical_document_meta_hash,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(label: &str) -> TempDirGuard {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let p = std::env::temp_dir().join(format!("drawcraft_engine_sync_{label}_{nanos}"));
        let _ = std::fs::remove_dir_all(&p);
        TempDirGuard(p)
    }

    fn fake_account(id: &str, email: &str, username: &str) -> ActiveAccountSession {
        ActiveAccountSession {
            server_url: "http://127.0.0.1:8787".to_string(),
            user: UserProfile {
                user_id: id.to_string(),
                email: Some(email.to_string()),
                username: username.to_string(),
                display_name: username.to_string(),
                created_at_ms: 1000,
                must_change_password: false,
            },
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            access_expires_at_ms: 9_999_999,
            refresh_expires_at_ms: 9_999_999,
            device_name: "TestPC".to_string(),
            last_auth_ms: 1000,
        }
    }

    fn apply_fake_upload_acks(
        session: &mut Session,
        batch: &drawcraft_sync::SyncBatch,
        synced_at_ms: i64,
    ) {
        let accepted: Vec<_> = batch
            .local_docs
            .iter()
            .enumerate()
            .map(|(i, snap)| drawcraft_sync_protocol::AcceptedNotebookSync {
                notebook_id: snap.notebook_id.clone(),
                new_version: snap.base_version + 1,
                cursor: (i + 1) as u64,
                deleted: false,
                content_hash: snap.content_hash.clone(),
                body_hash: canonical_document_body_hash(&snap.doc),
                meta_hash: canonical_document_meta_hash(&snap.doc),
            })
            .collect();
        let count = accepted.len();
        session
            .apply_sync_outcome(SyncCycleOutcome {
                new_cursor: count as u64,
                refreshed_session: None,
                server_user: fake_account("user-a", "alice@example.com", "alice").user,
                accepted_uploads: accepted,
                accepted_folders: Vec::new(),
                cleared_tombstones: Vec::new(),
                resolved_conflicts: Vec::new(),
                downloaded_updates: Vec::new(),
                downloaded_folders: Vec::new(),
                updated_prefs: None,
                ack_prefs: None,
                prefs_conflict: None,
                pushed_count: count,
                pulled_count: 0,
                conflict_count: 0,
                synced_at_ms,
            })
            .unwrap();
    }

    fn has_notebook_title(session: &Session, title: &str) -> bool {
        session.notebooks.iter().any(|n| n.title == title)
    }

    fn verify_reopened_tombstone_and_signout(dir_path: PathBuf, personal_nb_id: &str) {
        let mut reopened = Session::open_desktop_library(dir_path);
        assert!(reopened.is_viewing_cloud());
        assert_eq!(reopened.prepare_sync_batch().unwrap().tombstones.len(), 1);

        reopened.sign_out_to_local_library();
        assert!(!reopened.is_signed_in());
        assert!(!reopened.is_viewing_cloud());
        assert!(
            reopened
                .notebooks
                .iter()
                .any(|n| n.id == personal_nb_id && n.title == "Personal Offline Idea")
        );
        assert_eq!(reopened.local_profiles().len(), 2);
    }

    #[test]
    fn desktop_local_profiles_switch_isolate_and_preserve_tombstones() {
        let dir = temp_dir("isolation");
        let mut session = Session::open_desktop_library(dir.0.clone());
        assert!(!session.is_signed_in());
        assert!(!session.is_viewing_cloud());
        assert_eq!(session.local_profiles().len(), 1);
        let personal_id = session
            .active_local_profile()
            .unwrap()
            .local_profile_id
            .clone();
        assert_eq!(session.notebook_sync_status(0), EntitySyncStatus::LocalOnly);

        session
            .execute(
                "notebook.create",
                &json!({ "title": "Personal Offline Idea" }),
            )
            .unwrap();
        let personal_nb_id = session.doc.id.clone();
        assert!(!personal_nb_id.is_empty());

        // Create a second local profile ("Work") -> completely isolated.
        let _work = session.create_local_profile("Work", true).unwrap();
        assert_eq!(session.local_profiles().len(), 2);
        assert!(!has_notebook_title(&session, "Personal Offline Idea"));

        // Switch into Cloud account via SwitchOnly -> NEVER touches or duplicates local profiles!
        session.switch_to_authenticated_library(
            fake_account("user-a", "alice@example.com", "alice"),
            AuthWorkspaceMode::SwitchOnly,
        );
        assert!(session.is_signed_in());
        assert!(session.is_viewing_cloud());
        assert!(session.prepare_sync_batch().unwrap().local_docs.is_empty());

        session
            .execute("notebook.create", &json!({ "title": "Cloud Research" }))
            .unwrap();
        let cloud_idx = session.active_notebook;
        assert_eq!(
            session.notebook_sync_status(cloud_idx),
            EntitySyncStatus::PendingPush
        );

        let batch = session.prepare_sync_batch().unwrap();
        assert_eq!(batch.local_docs.len(), 1);
        apply_fake_upload_acks(&mut session, &batch, 2000);
        assert_eq!(
            session.notebook_sync_status(cloud_idx),
            EntitySyncStatus::Synced
        );
        assert_eq!(session.cloud_pending_changes_count(), 0);

        // Offline delete triggers tombstone and survives restart!
        session
            .execute("notebook.delete", &json!({ "index": cloud_idx }))
            .unwrap();
        assert_eq!(session.cloud_pending_changes_count(), 1);

        // Switch to Personal local profile WITHOUT signing out -> cloud pending changes still tracked!
        session.switch_to_local_profile(&personal_id).unwrap();
        assert!(session.is_signed_in());
        assert!(!session.is_viewing_cloud());
        assert_eq!(session.cloud_pending_changes_count(), 1);
        assert!(has_notebook_title(&session, "Personal Offline Idea"));

        // Switch back to Cloud, reopen from disk -> tombstone still intact, then sign out cleanly.
        session.switch_to_cloud_workspace().unwrap();
        drop(session);
        verify_reopened_tombstone_and_signout(dir.0.clone(), &personal_nb_id);
    }

    #[test]
    fn linking_and_manual_transfer_preserve_uuids_without_duplication() {
        let dir = temp_dir("link_and_transfer");
        let mut session = Session::open_desktop_library(dir.0.clone());
        let starter_count = session.notebooks.len();
        session
            .execute("notebook.create", &json!({ "title": "To Promote" }))
            .unwrap();
        let promoted_nb_id = session.doc.id.clone();
        let prof_id = session
            .active_local_profile()
            .unwrap()
            .local_profile_id
            .clone();

        // Create a second local profile ("Keep Local") before linking the first one.
        let keep_prof = session.create_local_profile("Keep Local", false).unwrap();

        // Promote first profile into Cloud -> keeps exact UUIDs, zero duplication, marks PendingPush.
        session.switch_to_authenticated_library(
            fake_account("user-link", "link@example.com", "linkuser"),
            AuthWorkspaceMode::PromoteLocalProfile {
                source_profile_id: prof_id,
            },
        );
        assert_eq!(session.notebooks.len(), starter_count + 1);
        assert!(session.notebooks.iter().any(|d| d.id == promoted_nb_id));
        assert_eq!(session.local_profiles().len(), 1);
        assert_eq!(
            session.local_profiles()[0].local_profile_id,
            keep_prof.local_profile_id
        );

        // Switch to "Keep Local" and create a notebook; Move it to Cloud -> same UUID!
        session
            .switch_to_local_profile(&keep_prof.local_profile_id)
            .unwrap();
        session
            .execute("notebook.create", &json!({ "title": "Moved Individual" }))
            .unwrap();
        let moved_id = session.doc.id.clone();
        let res_id = session
            .transfer_local_notebook_to_cloud(&keep_prof.local_profile_id, &moved_id, true)
            .unwrap();
        assert_eq!(res_id, moved_id);
        assert!(!session.notebooks.iter().any(|d| d.id == moved_id));

        // Copy another notebook to Cloud -> new UUID, original remains local.
        let copy_src_id = session.notebooks[0].id.clone();
        let copied_id = session
            .transfer_local_notebook_to_cloud(&keep_prof.local_profile_id, &copy_src_id, false)
            .unwrap();
        assert_ne!(copied_id, copy_src_id);
        assert!(session.notebooks.iter().any(|d| d.id == copy_src_id));

        session.switch_to_cloud_workspace().unwrap();
        assert!(session.notebooks.iter().any(|d| d.id == moved_id));
        assert!(session.notebooks.iter().any(|d| d.id == copied_id));
        assert!(session.notebooks.iter().any(|d| d.id == promoted_nb_id));
    }

    #[test]
    fn metadata_only_divergence_merges_lww_without_conflict_copy() {
        let dir = temp_dir("lww_metadata");
        let mut session = Session::open_desktop_library(dir.0.clone());
        let acct = fake_account("user-b", "bob@example.com", "bob");
        session.switch_to_authenticated_library(acct.clone(), AuthWorkspaceMode::SwitchOnly);

        let mut base = Document {
            id: "nb-lww".to_string(),
            title: "Original Title".to_string(),
            cover: NotebookCover::Terracotta,
            meta_updated_at_ms: 1000,
            ..Default::default()
        };
        let sid = base.alloc_stroke_id();
        let pts = vec![
            StrokeSample::from_xy_pressure(10.0, 10.0, 0.5),
            StrokeSample::from_xy_pressure(50.0, 50.0, 0.7),
        ];
        base.strokes.push(Arc::new(drawcraft_doc::InkStroke::new(
            sid,
            PenSettings::fountain_pen(),
            pts.clone(),
            pts,
        )));
        base.sync_active_to_pages();
        let base_hash = canonical_document_hash(&base);

        // Initial pull of nb-lww.
        session
            .apply_sync_outcome(SyncCycleOutcome {
                new_cursor: 1,
                refreshed_session: None,
                server_user: acct.user.clone(),
                accepted_uploads: Vec::new(),
                accepted_folders: Vec::new(),
                cleared_tombstones: Vec::new(),
                resolved_conflicts: Vec::new(),
                downloaded_updates: vec![RemoteNotebookUpdate {
                    notebook_id: "nb-lww".to_string(),
                    version: 1,
                    cursor: 1,
                    deleted: false,
                    updated_at_ms: 1000,
                    deleted_at_ms: None,
                    content_hash: base_hash,
                    body_hash: canonical_document_body_hash(&base),
                    meta_hash: canonical_document_meta_hash(&base),
                    meta_updated_at_ms: 1000,
                    document: Some(base.clone()),
                }],
                downloaded_folders: Vec::new(),
                updated_prefs: None,
                ack_prefs: None,
                prefs_conflict: None,
                pushed_count: 0,
                pulled_count: 1,
                conflict_count: 0,
                synced_at_ms: 1000,
            })
            .unwrap();

        // Local edits only metadata (`favorite = true`) at t=3000.
        session
            .execute(
                "notebook.setMetadata",
                &json!({ "index": 0, "favorite": true }),
            )
            .unwrap();
        {
            let mut d = (*session.notebooks[0]).clone();
            d.meta_updated_at_ms = 3000;
            session.notebooks[0] = Arc::new(d);
            session.doc = Arc::clone(&session.notebooks[0]);
        }

        // Remote edited metadata (`title = "Remote Title"`) at t=2000 (same body!).
        let mut remote_meta = base;
        remote_meta.title = "Remote Title".to_string();
        remote_meta.meta_updated_at_ms = 2000;
        let remote_hash = canonical_document_hash(&remote_meta);
        session
            .apply_sync_outcome(SyncCycleOutcome {
                new_cursor: 2,
                refreshed_session: None,
                server_user: acct.user,
                accepted_uploads: Vec::new(),
                accepted_folders: Vec::new(),
                cleared_tombstones: Vec::new(),
                resolved_conflicts: Vec::new(),
                downloaded_updates: vec![RemoteNotebookUpdate {
                    notebook_id: "nb-lww".to_string(),
                    version: 2,
                    cursor: 2,
                    deleted: false,
                    updated_at_ms: 2000,
                    deleted_at_ms: None,
                    content_hash: remote_hash,
                    body_hash: canonical_document_body_hash(&remote_meta),
                    meta_hash: canonical_document_meta_hash(&remote_meta),
                    meta_updated_at_ms: 2000,
                    document: Some(remote_meta),
                }],
                downloaded_folders: Vec::new(),
                updated_prefs: None,
                ack_prefs: None,
                prefs_conflict: None,
                pushed_count: 0,
                pulled_count: 1,
                conflict_count: 0,
                synced_at_ms: 2500,
            })
            .unwrap();

        // Zero conflict copy created, and newer local metadata won!
        assert_eq!(session.notebooks.len(), 1);
        assert!(session.doc.favorite);
        assert_eq!(session.doc.title, "Original Title");
    }

    #[test]
    fn web_cache_roundtrip_and_account_isolation_work_without_filesystem() {
        let mut session = Session::open_web_cache(None);
        assert!(session.is_sync_persistence_enabled());
        assert!(!session.is_signed_in());
        assert!(!session.needs_initial_nickname());
        let guest_start = session.notebooks.len();
        assert_eq!(guest_start, 0);

        session
            .execute("notebook.create", &json!({ "title": "Web Browser Note" }))
            .unwrap();
        assert_eq!(session.notebooks.len(), guest_start + 1);

        let cached_json = session.export_web_cache_json().unwrap();
        let mut restored = Session::open_web_cache(Some(&cached_json));
        assert!(
            restored
                .notebooks
                .iter()
                .any(|n| n.title == "Web Browser Note")
        );

        let acct = fake_account("web-u1", "web@example.com", "web_user");
        restored.switch_to_authenticated_library(acct.clone(), AuthWorkspaceMode::SwitchOnly);
        assert!(restored.is_signed_in());
        assert!(restored.is_viewing_cloud());
        assert!(
            !restored
                .notebooks
                .iter()
                .any(|n| n.title == "Web Browser Note")
        );
        restored
            .execute("notebook.create", &json!({ "title": "Account Cloud Note" }))
            .unwrap();

        let cached_signed_in = restored.export_web_cache_json().unwrap();
        let mut reloaded_signed_in = Session::open_web_cache(Some(&cached_signed_in));
        assert!(reloaded_signed_in.is_signed_in());
        assert!(
            reloaded_signed_in
                .notebooks
                .iter()
                .any(|n| n.title == "Account Cloud Note")
        );

        reloaded_signed_in.sign_out_to_local_library();
        assert!(!reloaded_signed_in.is_signed_in());
        assert!(
            reloaded_signed_in
                .notebooks
                .iter()
                .any(|n| n.title == "Web Browser Note")
        );
        assert!(
            !reloaded_signed_in
                .notebooks
                .iter()
                .any(|n| n.title == "Account Cloud Note")
        );
    }

    #[test]
    fn first_access_nickname_and_local_discovery_report_work_end_to_end() {
        let dir = temp_dir("first_access");
        let mut session = Session::open_desktop_library(dir.0.clone());
        assert!(session.needs_initial_nickname());
        let before = session.local_discovery_report();
        assert_eq!(before.unsynced_local_accounts, 0);
        assert_eq!(before.total_local_notebooks, 0);

        // Blank nickname must fail cleanly without closing prompt.
        assert!(
            session
                .execute("profile.completeFirstAccess", &json!({ "nickname": "   " }))
                .is_err()
        );
        assert!(session.needs_initial_nickname());

        session
            .execute(
                "profile.completeFirstAccess",
                &json!({ "nickname": "Bruno" }),
            )
            .unwrap();
        assert!(!session.needs_initial_nickname());
        assert_eq!(session.active_local_profile().unwrap().nickname, "Bruno");

        let after = session.local_discovery_report();
        assert_eq!(after.unsynced_local_accounts, 1);
        assert_eq!(after.total_local_notebooks, session.notebooks.len());

        // Reopening later must NOT re-prompt for first-access nickname.
        let reopened = Session::open_desktop_library(dir.0.clone());
        assert!(!reopened.needs_initial_nickname());
        assert_eq!(reopened.active_local_profile().unwrap().nickname, "Bruno");
    }

    #[test]
    fn package_export_and_import_restore_all_notebooks_trash_and_prefs() {
        let dir_a = temp_dir("pkg_src");
        let mut src = Session::open_desktop_library(dir_a.0.clone());
        src.execute(
            "profile.completeFirstAccess",
            &json!({ "nickname": "Bruno" }),
        )
        .unwrap();
        src.execute(
            "notebook.create",
            &json!({ "title": "Math", "folder_path": "School" }),
        )
        .unwrap();
        src.execute("notebook.create", &json!({ "title": "Draft To Trash" }))
            .unwrap();
        let trash_idx = src
            .notebooks
            .iter()
            .position(|d| d.title == "Draft To Trash")
            .unwrap();
        src.execute("notebook.delete", &json!({ "index": trash_idx }))
            .unwrap();
        src.execute("library.folder.create", &json!({ "path": "School" }))
            .unwrap();
        src.execute("theme.set", &json!({ "theme": "Dark" }))
            .unwrap();

        let pkg_path = dir_a.0.join("full_backup.dcraftpkg");
        let export_res = src
            .execute(
                "package.export",
                &json!({ "path": pkg_path.to_string_lossy() }),
            )
            .unwrap();
        assert!(export_res["bytes"].as_u64().unwrap() > 0);

        // Import into a completely separate fresh workspace.
        let dir_b = temp_dir("pkg_dst");
        let mut dst = Session::open_desktop_library(dir_b.0.clone());
        dst.execute("profile.completeFirstAccess", &json!({ "nickname": "Ada" }))
            .unwrap();
        let import_res = dst
            .execute(
                "package.import",
                &json!({ "path": pkg_path.to_string_lossy() }),
            )
            .unwrap();
        assert!(import_res["importedNotebooks"].as_u64().unwrap() >= 1);
        assert_eq!(import_res["importedTrashed"], 1);
        assert!(dst.notebooks.iter().any(|n| n.title == "Math"));
        assert!(
            dst.trashed_notebooks
                .iter()
                .any(|t| t.doc.title == "Draft To Trash")
        );
        assert!(dst.prefs.library_folders.iter().any(|f| f == "School"));
        assert_eq!(dst.prefs.theme, drawcraft_doc::UiTheme::Dark);
    }

    #[test]
    fn migrate_local_account_renames_notebooks_and_deletes_old_local_data() {
        let dir = temp_dir("migrate_local");
        let mut session = Session::open_desktop_library(dir.0.clone());
        session
            .execute(
                "profile.completeFirstAccess",
                &json!({ "nickname": "Bruno" }),
            )
            .unwrap();
        session
            .execute("notebook.create", &json!({ "title": "Math" }))
            .unwrap();
        let bruno_id = session
            .active_local_profile()
            .unwrap()
            .local_profile_id
            .clone();

        let acct = fake_account("user-ada", "ada@example.com", "Ada");
        session.switch_to_authenticated_library(acct, AuthWorkspaceMode::SwitchOnly);
        assert!(session.is_signed_in());
        assert_eq!(session.local_profiles().len(), 1);

        let res = session
            .execute(
                "sync.migrateLocalAccount",
                &json!({ "profile_id": bruno_id }),
            )
            .unwrap();
        assert_eq!(res["sourceNickname"], "Bruno");
        assert!(session.notebooks.iter().any(|d| d.title == "Math - Bruno"));
        assert!(session.local_profiles().is_empty());
        assert!(!drawcraft_sync::profile_library_dir(&dir.0, &bruno_id).exists());

        // Signing out cleanly when 0 local profiles remain starts a fresh local onboarding prompt.
        session.sign_out_to_local_library();
        assert!(!session.is_signed_in());
        assert!(session.needs_initial_nickname());
    }

    #[test]
    fn keep_server_version_discards_offline_edits_cleanly() {
        let dir = temp_dir("keep_server");
        let mut session = Session::open_desktop_library(dir.0.clone());
        let acct = fake_account("user-srv", "srv@example.com", "srv_user");
        session.switch_to_authenticated_library(acct.clone(), AuthWorkspaceMode::SwitchOnly);

        let srv_doc = Document {
            id: "nb-srv".to_string(),
            title: "Server Authoritative".to_string(),
            ..Default::default()
        };
        let srv_hash = canonical_document_hash(&srv_doc);

        session
            .apply_sync_outcome(SyncCycleOutcome {
                new_cursor: 1,
                refreshed_session: None,
                server_user: acct.user.clone(),
                accepted_uploads: Vec::new(),
                accepted_folders: Vec::new(),
                cleared_tombstones: Vec::new(),
                resolved_conflicts: Vec::new(),
                downloaded_updates: vec![RemoteNotebookUpdate {
                    notebook_id: "nb-srv".to_string(),
                    version: 1,
                    cursor: 1,
                    deleted: false,
                    updated_at_ms: 100,
                    deleted_at_ms: None,
                    content_hash: srv_hash.clone(),
                    body_hash: canonical_document_body_hash(&srv_doc),
                    meta_hash: canonical_document_meta_hash(&srv_doc),
                    meta_updated_at_ms: 100,
                    document: Some(srv_doc.clone()),
                }],
                downloaded_folders: Vec::new(),
                updated_prefs: None,
                ack_prefs: None,
                prefs_conflict: None,
                pushed_count: 0,
                pulled_count: 1,
                conflict_count: 0,
                synced_at_ms: 100,
            })
            .unwrap();

        // Simulate offline local edits and extra offline notebook creation.
        session.record_sync_failure(true, "server offline".to_string());
        session
            .execute(
                "notebook.create",
                &json!({ "title": "Unwanted Offline Note" }),
            )
            .unwrap();
        assert!(session.offline_edits_pending());

        // Resolving in favor of Server Version discards the offline edit and extra notebook.
        session
            .apply_keep_server_version(SyncCycleOutcome {
                new_cursor: 2,
                refreshed_session: None,
                server_user: acct.user,
                accepted_uploads: Vec::new(),
                accepted_folders: Vec::new(),
                cleared_tombstones: Vec::new(),
                resolved_conflicts: Vec::new(),
                downloaded_updates: vec![RemoteNotebookUpdate {
                    notebook_id: "nb-srv".to_string(),
                    version: 1,
                    cursor: 2,
                    deleted: false,
                    updated_at_ms: 200,
                    deleted_at_ms: None,
                    content_hash: srv_hash,
                    body_hash: canonical_document_body_hash(&srv_doc),
                    meta_hash: canonical_document_meta_hash(&srv_doc),
                    meta_updated_at_ms: 200,
                    document: Some(srv_doc),
                }],
                downloaded_folders: Vec::new(),
                updated_prefs: None,
                ack_prefs: None,
                prefs_conflict: None,
                pushed_count: 0,
                pulled_count: 1,
                conflict_count: 0,
                synced_at_ms: 200,
            })
            .unwrap();

        assert!(!session.offline_edits_pending());
        assert_eq!(session.notebooks.len(), 1);
        assert_eq!(session.notebooks[0].title, "Server Authoritative");
    }

    #[test]
    fn profile_commands_create_switch_rename_and_delete_local_profiles() {
        let dir = temp_dir("profile_cmds");
        let mut session = Session::open_desktop_library(dir.0.clone());
        session
            .execute(
                "profile.completeFirstAccess",
                &json!({ "nickname": "Primary" }),
            )
            .unwrap();
        let primary_id = session
            .active_local_profile()
            .unwrap()
            .local_profile_id
            .clone();
        session
            .execute("notebook.create", &json!({ "title": "Primary Note" }))
            .unwrap();

        // Create second profile with camelCase `switchNow` and verify immediate switch.
        let created_test = session
            .execute(
                "profile.create",
                &json!({ "nickname": "test", "switchNow": true }),
            )
            .unwrap();
        let test_id = created_test["localProfileId"].as_str().unwrap().to_string();
        assert_eq!(session.active_local_profile().unwrap().nickname, "test");
        assert!(!session.notebooks.iter().any(|n| n.title == "Primary Note"));
        session
            .execute("notebook.create", &json!({ "title": "Test Note" }))
            .unwrap();
        let test_nb_id = session
            .notebooks
            .iter()
            .find(|n| n.title == "Test Note")
            .unwrap()
            .id
            .clone();

        // Create third profile with snake_case `switch_now: false`.
        let created_test2 = session
            .execute(
                "profile.create",
                &json!({ "nickname": "test2", "switch_now": false }),
            )
            .unwrap();
        let test2_id = created_test2["localProfileId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(session.active_local_profile().unwrap().nickname, "test");

        // Switch via omitted `target` + camelCase `profileId`.
        session
            .execute("profile.switch", &json!({ "profileId": test2_id }))
            .unwrap();
        assert_eq!(session.active_local_profile().unwrap().nickname, "test2");
        assert!(!session.notebooks.iter().any(|n| n.title == "Test Note"));

        // Switch back via explicit `target: "local"` + snake_case `profile_id`.
        session
            .execute(
                "profile.switch",
                &json!({ "target": "local", "profile_id": primary_id }),
            )
            .unwrap();
        assert_eq!(session.active_local_profile().unwrap().nickname, "Primary");
        assert!(session.notebooks.iter().any(|n| n.title == "Primary Note"));

        // Rename via camelCase `profileId` and snake_case `profile_id`.
        session
            .execute(
                "profile.rename",
                &json!({ "profileId": test2_id, "nickname": "Work" }),
            )
            .unwrap();
        assert!(
            session
                .local_profiles()
                .iter()
                .any(|p| p.local_profile_id == test2_id && p.nickname == "Work")
        );

        // Delete via camelCase `profileId`.
        session
            .execute("profile.delete", &json!({ "profileId": test2_id }))
            .unwrap();
        assert!(
            !session
                .local_profiles()
                .iter()
                .any(|p| p.local_profile_id == test2_id)
        );

        // Connect cloud account, switch between local and cloud, and transfer notebook with camelCase params.
        let acct = fake_account("user-cloud", "cloud@example.com", "CloudUser");
        session.switch_to_authenticated_library(acct, AuthWorkspaceMode::SwitchOnly);
        assert!(session.is_viewing_cloud());

        session
            .execute(
                "profile.switch",
                &json!({ "target": "local", "profile_id": test_id }),
            )
            .unwrap();
        assert!(!session.is_viewing_cloud());
        assert_eq!(session.active_local_profile().unwrap().nickname, "test");

        session
            .execute("profile.switch", &json!({ "target": "cloud" }))
            .unwrap();
        assert!(session.is_viewing_cloud());

        session
            .execute(
                "sync.transferNotebook",
                &json!({
                    "sourceProfileId": test_id,
                    "notebookId": test_nb_id,
                    "move": false,
                }),
            )
            .unwrap();
        assert!(session.notebooks.iter().any(|n| n.title == "Test Note"));
    }

    #[test]
    fn local_discovery_report_counts_without_creating_missing_profile_dirs() {
        let dir = temp_dir("discovery_no_mkdir");
        let mut session = Session::open_desktop_library(dir.0.clone());
        session
            .execute(
                "profile.completeFirstAccess",
                &json!({ "nickname": "Primary" }),
            )
            .unwrap();
        let primary_count = session.notebooks.len();

        let uninitialized = session.create_local_profile("Unused", false).unwrap();
        let uninitialized_dir =
            drawcraft_sync::profile_library_dir(&dir.0, &uninitialized.local_profile_id);
        assert!(!uninitialized_dir.exists());

        let secondary = session.create_local_profile("School", true).unwrap();
        session
            .execute("notebook.create", &json!({ "title": "Calculus" }))
            .unwrap();
        let school_count = session.notebooks.len();

        let report = session.local_discovery_report();
        assert_eq!(report.unsynced_local_accounts, 3);
        assert_eq!(report.total_local_notebooks, primary_count + school_count);
        assert!(
            !uninitialized_dir.exists(),
            "local_discovery_report must never create missing profile directories on disk"
        );
        assert_ne!(secondary.local_profile_id, uninitialized.local_profile_id);
    }

    #[test]
    fn cloud_bound_local_profile_is_isolated_from_other_accounts_and_signed_out_sessions() {
        let dir = temp_dir("cloud_bound_local_isolation");
        let mut session = Session::open_desktop_library(dir.0.clone());
        session
            .execute(
                "profile.completeFirstAccess",
                &json!({ "nickname": "SharedOffline" }),
            )
            .unwrap();
        let shared_id = session
            .active_local_profile()
            .unwrap()
            .local_profile_id
            .clone();

        // User A signs in and creates a local profile inside the account: linked to user-a.
        let acct_a = fake_account("user-a", "ada@example.com", "ada");
        session.switch_to_authenticated_library(acct_a, AuthWorkspaceMode::SwitchOnly);
        let a_local = session.create_local_profile("AdaOffline", true).unwrap();
        assert_eq!(a_local.owner_cloud_user_id.as_deref(), Some("user-a"));
        session
            .execute("notebook.create", &json!({ "title": "Ada Private Local" }))
            .unwrap();
        let a_nb_id = session.doc.id.clone();
        assert!(
            session
                .accessible_local_profiles()
                .iter()
                .any(|p| p.local_profile_id == a_local.local_profile_id)
        );

        // Signed out: only unlinked profiles are visible.
        session.sign_out_to_local_library();
        assert!(
            !session
                .accessible_local_profiles()
                .iter()
                .any(|p| p.local_profile_id == a_local.local_profile_id)
        );
        assert_eq!(
            session.active_local_profile().unwrap().local_profile_id,
            shared_id
        );

        // User B never sees, lists, migrates, transfers from or switches to user A's profile.
        let acct_b = fake_account("user-b", "bob@example.com", "bob");
        session.switch_to_authenticated_library(acct_b, AuthWorkspaceMode::SwitchOnly);
        assert!(
            session
                .list_local_profile_notebooks(&a_local.local_profile_id)
                .is_empty()
        );
        assert!(
            session
                .migrate_local_account_to_cloud(&a_local.local_profile_id)
                .is_err()
        );
        assert!(
            session
                .transfer_local_notebook_to_cloud(&a_local.local_profile_id, &a_nb_id, true)
                .is_err()
        );
        assert!(
            session
                .switch_to_local_profile(&a_local.local_profile_id)
                .is_err()
        );
        assert_eq!(session.local_discovery_report().unsynced_local_accounts, 1);

        // User A signs back in: the profile and its notebook are intact.
        let acct_a2 = fake_account("user-a", "ada@example.com", "ada");
        session.switch_to_authenticated_library(acct_a2, AuthWorkspaceMode::SwitchOnly);
        assert!(
            session
                .list_local_profile_notebooks(&a_local.local_profile_id)
                .iter()
                .any(|n| n.notebook_id == a_nb_id)
        );
    }
}
