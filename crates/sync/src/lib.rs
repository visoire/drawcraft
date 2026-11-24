//! L4 offline-first sync engine, HTTP client, and local manifest store for DrawCraft.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

pub mod client;
pub mod engine;
pub mod store;
pub mod types;

pub use client::SyncHttpClient;
pub use engine::{
    SyncCycleDriver, build_conflict_preview_if_needed, build_prune_to_local_request,
    make_conflict_title, outcome_from_full_pull, run_sync_cycle, run_sync_cycle_keep_local,
};
pub use store::{
    account_library_dir, default_device_name, inspect_local_accounts_in_library, load_global_state,
    load_manifest, profile_library_dir, sanitize_account_dir_segment, sanitize_client_id,
    sanitize_profile_nickname, save_global_state, save_manifest, write_bytes_atomic,
};
pub use types::{
    ACCOUNTS_DIR, ActiveAccountSession, ActiveWorkspace, ConflictResolutionCopy, EntitySyncStatus,
    FolderSyncMeta, GLOBAL_SYNC_FILE, GlobalSyncState, LocalNotebookSnapshot, LocalProfile,
    NotebookSyncMeta, NotebookTombstone, OwnerId, PROFILES_DIR, RemoteNotebookUpdate,
    SYNC_MANIFEST_FILE, SyncBatch, SyncConflictPreview, SyncConnectionState, SyncCycleOutcome,
    SyncError, SyncManifest, SyncStatusSummary,
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;

    use drawcraft_doc::{Document, NotebookCover};
    use drawcraft_sync_protocol::canonical_document_hash;

    use crate::engine::{build_conflict_preview_if_needed, make_conflict_title};
    use crate::store::{
        account_library_dir, inspect_local_accounts_in_library, load_global_state, load_manifest,
        profile_library_dir, save_global_state, save_manifest,
    };
    use crate::types::{
        GlobalSyncState, LocalNotebookSnapshot, LocalProfile, SyncBatch, SyncManifest,
    };

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
        let p = std::env::temp_dir().join(format!("drawcraft_sync_test_{label}_{nanos}"));
        let _ = std::fs::remove_dir_all(&p);
        TempDirGuard(p)
    }

    #[test]
    fn global_state_and_manifest_roundtrip_and_dirtiness_tracking() {
        let dir = temp_dir("manifest_roundtrip");

        let mut global = GlobalSyncState::with_defaults("MyLaptop".to_string());
        global.server_url = "100.80.1.5:8787".to_string();
        save_global_state(&dir.0, &global).unwrap();

        let loaded_global = load_global_state(&dir.0);
        assert_eq!(loaded_global.server_url, "http://100.80.1.5:8787");
        assert_eq!(loaded_global.device_name, "MyLaptop");

        let acct_dir = account_library_dir(&dir.0, "user-abc/../../bad");
        assert!(
            acct_dir.ends_with("accounts\\user-abcbad")
                || acct_dir.ends_with("accounts/user-abcbad")
        );

        let mut manifest = SyncManifest::default();
        let doc = Document {
            title: "Geometry".to_string(),
            cover: NotebookCover::ClassicNavy,
            ..Default::default()
        };
        let hash = canonical_document_hash(&doc);

        manifest.mark_notebook_dirty("nb-local", &hash);
        assert_eq!(manifest.pending_changes_count(), 1);

        manifest.mark_notebook_synced("nb-local", 1, hash.clone(), &hash, 12345);
        assert_eq!(manifest.pending_changes_count(), 0);

        // Local edit changes hash -> becomes dirty without erasing synced server version!
        manifest.mark_notebook_dirty("nb-local", "new-hash-after-offline-edit");
        assert_eq!(manifest.pending_changes_count(), 1);
        assert_eq!(manifest.meta("nb-local").unwrap().server_version, 1);

        // Deleting a synced notebook creates a tombstone.
        manifest.record_local_delete("nb-local", 999);
        assert_eq!(manifest.tombstones.len(), 1);
        assert_eq!(manifest.tombstones[0].base_version, 1);

        // Deleting an unsynced local-only notebook leaves zero tombstones.
        manifest.mark_notebook_dirty("nb-never-synced", "abc");
        manifest.record_local_delete("nb-never-synced", 1000);
        assert_eq!(manifest.tombstones.len(), 1);

        save_manifest(&acct_dir, &manifest).unwrap();
        let loaded_manifest = load_manifest(&acct_dir);
        assert_eq!(loaded_manifest, manifest);
    }

    #[test]
    fn conflict_title_generation_avoids_repeated_suffixes() {
        assert_eq!(
            make_conflict_title("Math Notes"),
            "Math Notes (Conflict Copy)"
        );
        assert_eq!(
            make_conflict_title("Math Notes (Conflict Copy)"),
            "Math Notes (Conflict Copy) 2"
        );
        assert_eq!(
            make_conflict_title("   "),
            "Untitled Notebook (Conflict Copy)"
        );
    }

    #[test]
    fn inspect_local_accounts_in_library_returns_counts_only() {
        let dir = temp_dir("inspect_counts");
        assert!(inspect_local_accounts_in_library(&dir.0).is_none());

        let mut global = GlobalSyncState::with_defaults("Desk".to_string());
        global.client_id = "client-a".to_string();
        global.local_profiles = vec![
            LocalProfile {
                local_profile_id: "p1".to_string(),
                nickname: "BrunoSecret".to_string(),
                created_at_ms: 1,
                last_used_at_ms: 1,
                owner_cloud_user_id: None,
            },
            LocalProfile {
                local_profile_id: "p2".to_string(),
                nickname: "SecondSecret".to_string(),
                created_at_ms: 2,
                last_used_at_ms: 2,
                owner_cloud_user_id: None,
            },
            LocalProfile {
                local_profile_id: "p3-other".to_string(),
                nickname: "OtherCloudLocal".to_string(),
                created_at_ms: 3,
                last_used_at_ms: 3,
                owner_cloud_user_id: Some("other-cloud-user".to_string()),
            },
        ];
        save_global_state(&dir.0, &global).unwrap();

        let p1_dir = profile_library_dir(&dir.0, "p1");
        std::fs::create_dir_all(p1_dir.join("trash")).unwrap();
        std::fs::write(p1_dir.join("a.dcraft"), b"dummy").unwrap();
        std::fs::write(p1_dir.join("b.NCRAFT"), b"dummy").unwrap();
        // Trashed notebooks must not inflate active count.
        std::fs::write(p1_dir.join("trash").join("old.dcraft"), b"dummy").unwrap();

        let p2_dir = profile_library_dir(&dir.0, "p2");
        std::fs::create_dir_all(&p2_dir).unwrap();
        std::fs::write(p2_dir.join("c.dcraft"), b"dummy").unwrap();

        // A profile linked to another cloud account is never counted.
        let p3_dir = profile_library_dir(&dir.0, "p3-other");
        std::fs::create_dir_all(&p3_dir).unwrap();
        std::fs::write(p3_dir.join("d.dcraft"), b"dummy").unwrap();

        let (cid, summary) = inspect_local_accounts_in_library(&dir.0).unwrap();
        assert_eq!(cid, "client-a");
        assert_eq!(summary.unsynced_local_accounts, 2);
        assert_eq!(summary.total_local_notebooks, 3);

        // Before completing initial nickname, report 0/0.
        global.needs_initial_nickname = true;
        save_global_state(&dir.0, &global).unwrap();
        let (_, pending_summary) = inspect_local_accounts_in_library(&dir.0).unwrap();
        assert_eq!(pending_summary.unsynced_local_accounts, 0);
        assert_eq!(pending_summary.total_local_notebooks, 0);
    }

    #[test]
    fn conflict_preview_triggers_for_offline_edits_or_divergence() {
        let batch = SyncBatch {
            client_cursor: 5,
            device_name: "Laptop".to_string(),
            local_docs: vec![LocalNotebookSnapshot {
                notebook_id: "nb-1".to_string(),
                base_version: 2,
                cloud_dirty: true,
                content_hash: "local-h".to_string(),
                doc: Document {
                    id: "nb-1".to_string(),
                    title: "Physics".to_string(),
                    ..Default::default()
                },
            }],
            tombstones: Vec::new(),
            folder_changes: Vec::new(),
            prefs_push: None,
            force_overwrite: false,
        };
        let pull = drawcraft_sync_protocol::SyncPullResponse {
            server_cursor: 5,
            user: drawcraft_sync_protocol::UserProfile {
                user_id: "u1".to_string(),
                email: Some("ada@example.com".to_string()),
                username: "Ada".to_string(),
                display_name: "Ada".to_string(),
                created_at_ms: 1,
                must_change_password: false,
            },
            notebooks: Vec::new(),
            folders: Vec::new(),
            studio_prefs: None,
        };
        // Normal online edit at same server cursor -> silent sync (no prompt).
        assert!(
            build_conflict_preview_if_needed("Ada", "ada@example.com", false, &batch, &pull)
                .is_none()
        );
        // Same cursor but offline edits were recorded -> must prompt.
        let preview =
            build_conflict_preview_if_needed("Ada", "ada@example.com", true, &batch, &pull)
                .expect("offline edit conflict preview");
        assert!(preview.offline_edits_detected);
        assert_eq!(preview.local_dirty_titles, vec!["Physics".to_string()]);
    }

    #[test]
    fn write_bytes_atomic_handles_concurrent_threads_without_leaving_temp_files() {
        let dir = temp_dir("atomic_concurrent");
        std::fs::create_dir_all(&dir.0).unwrap();
        let target = dir.0.join("notebook.dcraft");

        let payloads: Vec<Vec<u8>> = (0..4_u8)
            .map(|tid| vec![b'A' + tid; 16 * 1024 + usize::from(tid) * 37])
            .collect();

        let mut handles = Vec::new();
        for (tid, payload) in payloads.iter().enumerate() {
            let path = target.clone();
            let bytes = payload.clone();
            handles.push(std::thread::spawn(move || {
                for iter in 0..12 {
                    let mut buf = bytes.clone();
                    buf[0] = b'0' + (tid as u8);
                    if let Some(last) = buf.last_mut() {
                        *last = b'a' + ((iter % 26) as u8);
                    }
                    crate::write_bytes_atomic(&path, &buf).expect("atomic write should succeed");
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let final_bytes = std::fs::read(&target).unwrap();
        assert!(
            final_bytes.len() >= 16 * 1024,
            "final file should never be truncated"
        );
        let leftovers: Vec<String> = std::fs::read_dir(&dir.0)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "unexpected leftover temp files: {leftovers:?}"
        );
    }
}
