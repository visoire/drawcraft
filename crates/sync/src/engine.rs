use std::io::{self, Write};
use std::ops::Range;

use drawcraft_sync_protocol::{
    AcceptedFolderSync, ConflictNotebookSync, LocalNotebookChange, MAX_PUSH_BATCH_SIZE,
    PruneToLocalRequest, RemoteFolderRecord, RemoteNotebookRecord, RemotePrefsRecord,
    SyncPullResponse, SyncPushRequest, SyncPushResponse, canonical_document_body_hash,
    canonical_document_hash, canonical_document_meta_hash, now_unix_ms,
};
use uuid::Uuid;

use crate::client::SyncHttpClient;
use crate::types::{
    ActiveAccountSession, ConflictResolutionCopy, LocalNotebookSnapshot, RemoteNotebookUpdate,
    SyncBatch, SyncConflictPreview, SyncCycleOutcome, SyncError,
};

const TARGET_PUSH_CHUNK_BYTES: usize = 24 * 1024 * 1024;

pub fn make_conflict_title(original_title: &str) -> String {
    let base = original_title.trim();
    if base.is_empty() {
        "Untitled Notebook (Conflict Copy)".to_string()
    } else if base.ends_with(" (Conflict Copy)") {
        format!("{base} 2")
    } else {
        format!("{base} (Conflict Copy)")
    }
}

/// Pure step-by-step sync state machine shared by blocking desktop HTTP and async web `fetch`.
pub struct SyncCycleDriver {
    changes: Vec<LocalNotebookChange>,
    ranges: Vec<Range<usize>>,
    cursor: usize,
    prefs_only_pending: bool,
    accum: PushAccum,
}

impl SyncCycleDriver {
    /// Prepares chunked push requests from a captured local [`SyncBatch`].
    pub fn new(batch: &SyncBatch) -> Self {
        let changes = build_pending_changes(batch);
        let ranges = plan_push_chunk_ranges(&changes, MAX_PUSH_BATCH_SIZE, TARGET_PUSH_CHUNK_BYTES);
        let prefs_only_pending =
            changes.is_empty() && (batch.prefs_push.is_some() || !batch.folder_changes.is_empty());
        Self {
            changes,
            ranges,
            cursor: 0,
            prefs_only_pending,
            accum: PushAccum::default(),
        }
    }

    /// Returns the next chunked push request, or `None` once all push requests have completed.
    pub fn next_push_request(&self, batch: &SyncBatch) -> Option<SyncPushRequest> {
        if self.prefs_only_pending {
            return Some(SyncPushRequest {
                client_cursor: batch.client_cursor,
                device_name: batch.device_name.clone(),
                changes: Vec::new(),
                folders: batch.folder_changes.clone(),
                studio_prefs: batch.prefs_push.clone(),
                force_overwrite: batch.force_overwrite,
            });
        }
        let range = self.ranges.get(self.cursor)?.clone();
        let is_first = self.cursor == 0;
        let prefs_for_chunk = if is_first {
            batch.prefs_push.clone()
        } else {
            None
        };
        let folders_for_chunk = if is_first {
            batch.folder_changes.clone()
        } else {
            Vec::new()
        };
        Some(SyncPushRequest {
            client_cursor: batch.client_cursor,
            device_name: batch.device_name.clone(),
            changes: self.changes[range].to_vec(),
            folders: folders_for_chunk,
            studio_prefs: prefs_for_chunk,
            force_overwrite: batch.force_overwrite,
        })
    }

    /// Records the server's push response and advances to the next chunk.
    pub fn record_push_chunk_response(&mut self, resp: SyncPushResponse, batch: &SyncBatch) {
        let sent_prefs = if self.prefs_only_pending || self.cursor == 0 {
            batch.prefs_push.as_ref()
        } else {
            None
        };
        record_push_response(resp, batch, sent_prefs, &mut self.accum);
        if self.prefs_only_pending {
            self.prefs_only_pending = false;
        } else if self.cursor < self.ranges.len() {
            self.cursor += 1;
        }
    }

    /// Merges the final pull payload with recorded push acknowledgements into [`SyncCycleOutcome`].
    pub fn finish(
        mut self,
        pull_resp: SyncPullResponse,
        refreshed_session: Option<ActiveAccountSession>,
    ) -> SyncCycleOutcome {
        merge_pulled_records(
            &pull_resp.notebooks,
            &self.accum.accepted_uploads,
            &self.accum.cleared_tombstones,
            &mut self.accum.downloaded_updates,
        );
        let downloaded_folders: Vec<RemoteFolderRecord> = pull_resp
            .folders
            .into_iter()
            .filter(|rf| {
                !self
                    .accum
                    .accepted_folders
                    .iter()
                    .any(|af| af.folder_id == rf.folder_id && af.new_version == rf.version)
            })
            .collect();
        let updated_prefs =
            select_pulled_prefs(pull_resp.studio_prefs, self.accum.ack_prefs.as_ref());
        let new_cursor = pull_resp.server_cursor.max(self.accum.highest_cursor);

        SyncCycleOutcome {
            new_cursor,
            refreshed_session,
            server_user: pull_resp.user,
            accepted_uploads: self.accum.accepted_uploads,
            accepted_folders: self.accum.accepted_folders,
            cleared_tombstones: self.accum.cleared_tombstones,
            resolved_conflicts: self.accum.resolved_conflicts,
            pulled_count: self.accum.downloaded_updates.len(),
            downloaded_updates: self.accum.downloaded_updates,
            downloaded_folders,
            updated_prefs,
            ack_prefs: self.accum.ack_prefs,
            prefs_conflict: self.accum.prefs_conflict,
            pushed_count: self.accum.pushed_count,
            conflict_count: self.accum.conflict_count,
            synced_at_ms: now_unix_ms(),
        }
    }
}

pub fn run_sync_cycle(
    client: &SyncHttpClient,
    session: &mut ActiveAccountSession,
    batch: &SyncBatch,
) -> Result<SyncCycleOutcome, SyncError> {
    let initial_refresh = session.access_token.clone();
    let mut driver = SyncCycleDriver::new(batch);

    while let Some(req) = driver.next_push_request(batch) {
        let (resp, _) = client.push_with_refresh(session, &req)?;
        driver.record_push_chunk_response(resp, batch);
    }
    let (pull_resp, refreshed_on_pull) = client.pull_with_refresh(session, batch.client_cursor)?;
    let token_changed = refreshed_on_pull || session.access_token != initial_refresh;

    Ok(driver.finish(pull_resp, token_changed.then(|| session.clone())))
}

/// Authoritatively pushes the local version with `force_overwrite` and prunes any
/// unchosen server notebooks/folders not present in `batch`.
pub fn run_sync_cycle_keep_local(
    client: &SyncHttpClient,
    session: &mut ActiveAccountSession,
    batch: &SyncBatch,
) -> Result<SyncCycleOutcome, SyncError> {
    let mut forced = batch.clone();
    forced.force_overwrite = true;
    let initial_refresh = session.access_token.clone();
    let mut driver = SyncCycleDriver::new(&forced);

    while let Some(req) = driver.next_push_request(&forced) {
        let (resp, _) = client.push_with_refresh(session, &req)?;
        driver.record_push_chunk_response(resp, &forced);
    }

    let prune_req = build_prune_to_local_request(&forced);
    let (prune_resp, _) = client.prune_to_local_with_refresh(session, &prune_req)?;
    let (pull_resp, refreshed_on_pull) = client.pull_with_refresh(session, 0)?;
    let token_changed = refreshed_on_pull || session.access_token != initial_refresh;
    let mut outcome = driver.finish(pull_resp, token_changed.then(|| session.clone()));
    outcome.new_cursor = outcome.new_cursor.max(prune_resp.server_cursor);
    Ok(outcome)
}

/// Builds a prune request retaining only the notebooks and folders present locally.
#[must_use]
pub fn build_prune_to_local_request(batch: &SyncBatch) -> PruneToLocalRequest {
    PruneToLocalRequest {
        keep_notebook_ids: batch
            .local_docs
            .iter()
            .map(|d| d.notebook_id.clone())
            .collect(),
        keep_folder_ids: batch
            .folder_changes
            .iter()
            .filter(|f| !f.deleted)
            .map(|f| f.folder_id.clone())
            .collect(),
        device_name: batch.device_name.clone(),
    }
}

/// Converts a full `since=0` pull response into a clean [`SyncCycleOutcome`] for
/// "Keep Server Version" conflict resolution (discarding unchosen local edits).
#[must_use]
pub fn outcome_from_full_pull(
    pull_resp: SyncPullResponse,
    refreshed_session: Option<ActiveAccountSession>,
) -> SyncCycleOutcome {
    let downloaded_updates: Vec<RemoteNotebookUpdate> = pull_resp
        .notebooks
        .into_iter()
        .map(|record| RemoteNotebookUpdate {
            notebook_id: record.notebook_id,
            version: record.version,
            cursor: record.cursor,
            deleted: record.deleted,
            updated_at_ms: record.updated_at_ms,
            deleted_at_ms: record.deleted_at_ms,
            content_hash: record.content_hash,
            body_hash: record.body_hash,
            meta_hash: record.meta_hash,
            meta_updated_at_ms: record.meta_updated_at_ms,
            document: record.document,
        })
        .collect();
    SyncCycleOutcome {
        new_cursor: pull_resp.server_cursor,
        refreshed_session,
        server_user: pull_resp.user,
        accepted_uploads: Vec::new(),
        accepted_folders: Vec::new(),
        cleared_tombstones: Vec::new(),
        resolved_conflicts: Vec::new(),
        pulled_count: downloaded_updates.len(),
        downloaded_updates,
        downloaded_folders: pull_resp.folders,
        updated_prefs: pull_resp.studio_prefs,
        ack_prefs: None,
        prefs_conflict: None,
        pushed_count: 0,
        conflict_count: 0,
        synced_at_ms: now_unix_ms(),
    }
}

/// Detects whether offline edits or server/local divergence require user resolution
/// between **Server Version** and **Local Version**.
#[must_use]
pub fn build_conflict_preview_if_needed(
    account_name: &str,
    account_email: &str,
    offline_edits_pending: bool,
    batch: &SyncBatch,
    server_pull: &SyncPullResponse,
) -> Option<SyncConflictPreview> {
    let server_active: Vec<&RemoteNotebookRecord> = server_pull
        .notebooks
        .iter()
        .filter(|r| !r.deleted)
        .collect();
    let server_has_history = server_pull.server_cursor > 0
        || !server_pull.notebooks.is_empty()
        || !server_pull.folders.is_empty()
        || server_pull.studio_prefs.is_some();

    let local_pending_changes = batch
        .local_docs
        .iter()
        .filter(|d| d.cloud_dirty || d.base_version == 0)
        .count()
        .saturating_add(batch.tombstones.len())
        .saturating_add(usize::from(batch.prefs_push.is_some()));

    let has_local_edits = offline_edits_pending || local_pending_changes > 0;
    let offline_conflict = offline_edits_pending && server_has_history;
    let diverged_conflict = has_local_edits
        && (server_pull.server_cursor > batch.client_cursor
            || (batch.client_cursor == 0 && !server_active.is_empty()));

    if !offline_conflict && !diverged_conflict {
        return None;
    }

    let mut local_dirty_titles: Vec<String> = batch
        .local_docs
        .iter()
        .filter(|d| d.cloud_dirty || d.base_version == 0)
        .map(|d| d.doc.title.clone())
        .take(5)
        .collect();
    if local_dirty_titles.is_empty() {
        local_dirty_titles = batch
            .local_docs
            .iter()
            .map(|d| d.doc.title.clone())
            .take(5)
            .collect();
    }

    let server_notebook_titles: Vec<String> = server_active
        .iter()
        .filter_map(|r| r.document.as_ref().map(|d| d.title.clone()))
        .take(5)
        .collect();

    Some(SyncConflictPreview {
        account_name: account_name.to_string(),
        account_email: account_email.to_string(),
        server_notebook_count: server_active.len(),
        local_notebook_count: batch.local_docs.len(),
        server_cursor: server_pull.server_cursor,
        local_pending_changes: local_pending_changes.max(usize::from(offline_edits_pending)),
        local_dirty_titles,
        server_notebook_titles,
        offline_edits_detected: offline_edits_pending,
    })
}

#[derive(Default)]
struct PushAccum {
    accepted_uploads: Vec<drawcraft_sync_protocol::AcceptedNotebookSync>,
    accepted_folders: Vec<AcceptedFolderSync>,
    cleared_tombstones: Vec<String>,
    resolved_conflicts: Vec<ConflictResolutionCopy>,
    downloaded_updates: Vec<RemoteNotebookUpdate>,
    ack_prefs: Option<(u64, String)>,
    prefs_conflict: Option<RemotePrefsRecord>,
    highest_cursor: u64,
    pushed_count: usize,
    conflict_count: usize,
}

fn build_pending_changes(batch: &SyncBatch) -> Vec<LocalNotebookChange> {
    let mut changes = Vec::new();
    let now = now_unix_ms();

    for snap in &batch.local_docs {
        if !batch.force_overwrite && !snap.cloud_dirty && snap.base_version > 0 {
            continue;
        }
        let mut doc = snap.doc.clone();
        if doc.id.is_empty() {
            doc.id = snap.notebook_id.clone();
        }
        let body_hash = canonical_document_body_hash(&doc);
        let meta_hash = canonical_document_meta_hash(&doc);
        let updated_at_ms = doc.updated_at_ms.max(now);
        let meta_updated_at_ms = doc.meta_updated_at_ms.max(updated_at_ms);
        changes.push(LocalNotebookChange {
            notebook_id: snap.notebook_id.clone(),
            base_version: snap.base_version,
            deleted: false,
            updated_at_ms,
            deleted_at_ms: None,
            content_hash: snap.content_hash.clone(),
            body_hash,
            meta_hash,
            meta_updated_at_ms,
            document: Some(doc),
        });
    }

    for tomb in &batch.tombstones {
        changes.push(LocalNotebookChange {
            notebook_id: tomb.notebook_id.clone(),
            base_version: tomb.base_version,
            deleted: true,
            updated_at_ms: tomb.deleted_at_ms,
            deleted_at_ms: Some(tomb.deleted_at_ms),
            content_hash: "deleted".to_string(),
            body_hash: String::new(),
            meta_hash: String::new(),
            meta_updated_at_ms: tomb.deleted_at_ms,
            document: None,
        });
    }
    changes
}

struct ByteCounter(usize);

impl Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn estimate_change_bytes(change: &LocalNotebookChange) -> usize {
    let mut counter = ByteCounter(0);
    if serde_json::to_writer(&mut counter, change).is_ok() {
        counter.0.max(1)
    } else {
        1
    }
}

fn plan_push_chunk_ranges(
    changes: &[LocalNotebookChange],
    max_count: usize,
    max_bytes: usize,
) -> Vec<Range<usize>> {
    if changes.is_empty() {
        return Vec::new();
    }
    let count_cap = max_count.max(1);
    let byte_cap = max_bytes.max(1);
    let mut ranges = Vec::new();
    let mut start = 0;

    while start < changes.len() {
        let mut end = start;
        let mut chunk_bytes = 0usize;
        while end < changes.len() && (end - start) < count_cap {
            let item_bytes = estimate_change_bytes(&changes[end]);
            if end > start && chunk_bytes.saturating_add(item_bytes) > byte_cap {
                break;
            }
            chunk_bytes = chunk_bytes.saturating_add(item_bytes);
            end += 1;
        }
        ranges.push(start..end);
        start = end;
    }

    ranges
}

fn record_push_response(
    resp: SyncPushResponse,
    batch: &SyncBatch,
    sent_prefs: Option<&drawcraft_sync_protocol::LocalPrefsPush>,
    accum: &mut PushAccum,
) {
    accum.highest_cursor = accum.highest_cursor.max(resp.server_cursor);
    if let Some(conflict) = resp.prefs_conflict {
        accum.prefs_conflict = Some(conflict);
    } else if let Some(prefs) = sent_prefs {
        accum.ack_prefs = Some((resp.prefs_version, prefs.content_hash.clone()));
    }

    for acc in resp.accepted {
        accum.pushed_count += 1;
        if acc.deleted {
            accum.cleared_tombstones.push(acc.notebook_id);
        } else {
            accum.accepted_uploads.push(acc);
        }
    }
    accum.accepted_folders.extend(resp.accepted_folders);

    for conflict in resp.conflicts {
        handle_conflict(&conflict, &batch.local_docs, accum);
    }
}

fn handle_conflict(
    conflict: &ConflictNotebookSync,
    local_docs: &[LocalNotebookSnapshot],
    accum: &mut PushAccum,
) {
    let remote = &conflict.server_record;
    let local_match = local_docs
        .iter()
        .find(|snap| snap.notebook_id == conflict.notebook_id);

    if let Some(local) = local_match {
        let identical_content = !remote.deleted && remote.content_hash == local.content_hash;
        let same_body = !remote.deleted
            && !remote.body_hash.is_empty()
            && remote.body_hash == canonical_document_body_hash(&local.doc);
        if !identical_content && !same_body {
            accum.conflict_count += 1;
            let conflict_id = Uuid::new_v4().to_string();
            let mut copy_doc = local.doc.clone();
            let conflict_title = make_conflict_title(&copy_doc.title);
            copy_doc.id = conflict_id.clone();
            copy_doc.title = conflict_title.clone();
            let local_hash = canonical_document_hash(&copy_doc);
            accum.resolved_conflicts.push(ConflictResolutionCopy {
                original_notebook_id: conflict.notebook_id.clone(),
                conflict_notebook_id: conflict_id,
                conflict_title,
                local_doc: copy_doc,
                local_hash,
            });
        }
    } else if !remote.deleted {
        // Local tombstone conflicted with newer server edit -> clear tombstone & apply remote.
        accum.conflict_count += 1;
        accum.cleared_tombstones.push(conflict.notebook_id.clone());
    } else {
        accum.cleared_tombstones.push(conflict.notebook_id.clone());
    }

    upsert_downloaded_update(remote, &mut accum.downloaded_updates);
}

fn merge_pulled_records(
    records: &[RemoteNotebookRecord],
    accepted: &[drawcraft_sync_protocol::AcceptedNotebookSync],
    cleared_tombstones: &[String],
    target: &mut Vec<RemoteNotebookUpdate>,
) {
    for record in records {
        let is_own_upload = !record.deleted
            && accepted.iter().any(|acc| {
                acc.notebook_id == record.notebook_id && acc.new_version == record.version
            });
        let is_own_delete = record.deleted
            && cleared_tombstones
                .iter()
                .any(|id| id == &record.notebook_id);
        if is_own_upload || is_own_delete {
            continue;
        }
        upsert_downloaded_update(record, target);
    }
}

fn upsert_downloaded_update(record: &RemoteNotebookRecord, target: &mut Vec<RemoteNotebookUpdate>) {
    let candidate = RemoteNotebookUpdate {
        notebook_id: record.notebook_id.clone(),
        version: record.version,
        cursor: record.cursor,
        deleted: record.deleted,
        updated_at_ms: record.updated_at_ms,
        deleted_at_ms: record.deleted_at_ms,
        content_hash: record.content_hash.clone(),
        body_hash: record.body_hash.clone(),
        meta_hash: record.meta_hash.clone(),
        meta_updated_at_ms: record.meta_updated_at_ms,
        document: record.document.clone(),
    };

    if let Some(existing) = target
        .iter_mut()
        .find(|u| u.notebook_id == candidate.notebook_id)
    {
        if candidate.version >= existing.version {
            *existing = candidate;
        }
        return;
    }
    target.push(candidate);
}

fn select_pulled_prefs(
    remote: Option<RemotePrefsRecord>,
    ack_prefs: Option<&(u64, String)>,
) -> Option<RemotePrefsRecord> {
    let remote = remote?;
    if let Some((acked_ver, acked_hash)) = ack_prefs
        && remote.version <= *acked_ver
        && remote.content_hash == *acked_hash
    {
        return None;
    }
    Some(remote)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use drawcraft_doc::Document;

    #[test]
    fn plan_push_chunk_ranges_respects_byte_budget_while_keeping_single_large_items() {
        let small = LocalNotebookChange {
            notebook_id: "small".to_string(),
            base_version: 0,
            deleted: false,
            updated_at_ms: 1,
            deleted_at_ms: None,
            content_hash: "h".to_string(),
            body_hash: String::new(),
            meta_hash: String::new(),
            meta_updated_at_ms: 1,
            document: Some(Document::default()),
        };
        let single_bytes = estimate_change_bytes(&small);
        assert!(single_bytes > 50);

        let changes = vec![small.clone(), small.clone(), small];
        // Tight byte budget forces 1 item per chunk even though max_count is 64.
        let ranges = plan_push_chunk_ranges(&changes, 64, single_bytes + 10);
        assert_eq!(ranges, vec![0..1, 1..2, 2..3]);

        // Even when budget is smaller than 1 item, progress is still guaranteed.
        let oversized_ranges = plan_push_chunk_ranges(&changes, 64, 1);
        assert_eq!(oversized_ranges, vec![0..1, 1..2, 2..3]);
    }
}
