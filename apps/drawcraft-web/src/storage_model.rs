//! Browser persistence policy: what "saved" means for the workspace blob, kept free of any
//! browser API so it is unit-tested natively. `web_storage.rs` connects it to OPFS / IndexedDB.

use drawcraft_storage::{
    Budget, FlushBatch, FlushId, FlushOutcome, SaveStatus, StorageError, StorageKey, TierStore,
};
use drawcraft_ui::StorageIndicator;

/// Key of the serialized workspace (all notebooks plus preferences).
pub const WORKSPACE_KEY: (&str, &str, &str) = ("drawcraft", "web", "workspace");

/// Budgets for an iPadOS Safari tab. A workspace larger than this is refused (and the user is
/// told to export) instead of growing the WASM heap until the tab is killed.
pub const WORKSPACE_BUDGET: Budget = Budget {
    hot_bytes: 96 * 1024 * 1024,
    pending_bytes: 96 * 1024 * 1024,
    in_flight_bytes: 96 * 1024 * 1024,
    cold_bytes: None,
};

/// Maps a `DOMException` name (plus message) from the JS adapter to a [`StorageError`].
#[must_use]
pub fn error_from_js(name: &str, message: &str) -> StorageError {
    match name {
        "QuotaExceededError" | "NS_ERROR_DOM_QUOTA_REACHED" => StorageError::QuotaExceeded,
        "NotAllowedError" | "SecurityError" => StorageError::PermissionDenied,
        _ => StorageError::Backend(if message.is_empty() {
            name.to_string()
        } else {
            format!("{name}: {message}")
        }),
    }
}

/// Tracks the workspace blob through accept (step one) and durable persistence (step two).
pub struct StorageController {
    store: TierStore,
    key: StorageKey,
    last_error: Option<StorageError>,
    backend: String,
}

impl StorageController {
    /// `backend` is the adapter's choice (`opfs`, `indexeddb` or `none`).
    pub fn new(budget: Budget, backend: &str) -> Result<Self, StorageError> {
        let key = StorageKey::new(WORKSPACE_KEY.0, WORKSPACE_KEY.1, WORKSPACE_KEY.2)?;
        let mut ctl = Self {
            store: TierStore::new(budget),
            key,
            last_error: None,
            backend: backend.to_string(),
        };
        if backend == "none" {
            ctl.last_error = Some(StorageError::Backend(
                "no persistent browser storage is available".to_string(),
            ));
        }
        Ok(ctl)
    }

    #[must_use]
    pub fn key(&self) -> &StorageKey {
        &self.key
    }

    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Updates the cold-tier capacity from `navigator.storage.estimate()`.
    pub fn apply_estimate(&mut self, quota: u64, usage: u64) {
        if quota > 0 {
            // `Budget` is `Copy`; rebuilding the store keeps its (empty or hydrated) mirror semantics
            // simple, so the estimate is only applied before any data is accepted.
            let budget = self.store.budget().with_storage_estimate(quota, usage);
            if self.store.stats().hot_bytes == 0 {
                self.store = TierStore::new(budget);
            }
        }
    }

    /// Loads bytes read from persistence as the clean starting state.
    pub fn hydrate(&mut self, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.store.hydrate(&self.key, bytes).map(|_| ())
    }

    /// Step one: accept the serialized workspace. Refusals (too large, quota) are remembered
    /// so the UI can show the export fallback.
    pub fn save(&mut self, bytes: Vec<u8>) {
        match self.store.put(&self.key, bytes) {
            Ok(_) => {
                if matches!(
                    self.last_error,
                    Some(StorageError::TooLarge { .. } | StorageError::Backpressure { .. })
                ) {
                    self.last_error = None;
                }
            }
            Err(err) => self.last_error = Some(err),
        }
    }

    /// Hands pending bytes to the adapter.
    pub fn next_batch(&mut self) -> Option<FlushBatch> {
        self.store.begin_flush()
    }

    /// Step two: applies adapter results. Returns `true` when something must be retried.
    pub fn finish(&mut self, id: FlushId, outcomes: Vec<FlushOutcome>) -> bool {
        let report = self.store.complete_flush(id, outcomes);
        if let Some((_, _, err)) = report.retry.into_iter().next() {
            self.last_error = Some(err);
            return true;
        }
        if !report.saved.is_empty() || !report.superseded.is_empty() {
            self.last_error = None;
        }
        false
    }

    /// `true` while an accepted write waits to be (or is being) persisted.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.store.is_clean()
    }

    /// Only [`StorageIndicator::Saved`] may be shown as "Saved": it requires a durable write.
    #[must_use]
    pub fn indicator(&self) -> StorageIndicator {
        match &self.last_error {
            Some(
                StorageError::QuotaExceeded
                | StorageError::TooLarge { .. }
                | StorageError::Backpressure { .. },
            ) => return StorageIndicator::Full,
            Some(other) => return StorageIndicator::Failed(other.to_string()),
            None => {}
        }
        match self.store.status(&self.key) {
            SaveStatus::NotStored | SaveStatus::Saved { .. } => StorageIndicator::Saved,
            SaveStatus::Unsaved { .. } => StorageIndicator::Unsaved,
            SaveStatus::Saving { .. } => StorageIndicator::Saving,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(batch: &FlushBatch) -> Vec<FlushOutcome> {
        batch
            .writes
            .iter()
            .map(|w| FlushOutcome {
                key: w.key.clone(),
                revision: w.revision,
                result: Ok(()),
            })
            .collect()
    }

    fn failing(batch: &FlushBatch, err: StorageError) -> Vec<FlushOutcome> {
        batch
            .writes
            .iter()
            .map(|w| FlushOutcome {
                key: w.key.clone(),
                revision: w.revision,
                result: Err(err.clone()),
            })
            .collect()
    }

    fn small() -> Budget {
        Budget {
            hot_bytes: 1000,
            pending_bytes: 1000,
            in_flight_bytes: 1000,
            cold_bytes: None,
        }
    }

    #[test]
    fn saved_is_shown_only_after_the_adapter_confirms_the_write() {
        let mut c = StorageController::new(small(), "indexeddb").unwrap();
        assert_eq!(
            c.indicator(),
            StorageIndicator::Saved,
            "nothing to save yet"
        );
        c.save(vec![1; 100]);
        assert_eq!(c.indicator(), StorageIndicator::Unsaved);
        let batch = c.next_batch();
        assert!(batch.is_some());
        let batch = batch.unwrap();
        assert_eq!(c.indicator(), StorageIndicator::Saving);
        assert!(!c.finish(batch.id, ok(&batch)));
        assert_eq!(c.indicator(), StorageIndicator::Saved);
        assert!(!c.has_pending());
    }

    #[test]
    fn an_edit_during_a_flush_keeps_the_indicator_honest() {
        let mut c = StorageController::new(small(), "opfs").unwrap();
        c.save(vec![1; 10]);
        let first = c.next_batch().unwrap();
        c.save(vec![2; 10]);
        assert!(!c.finish(first.id, ok(&first)));
        assert_ne!(
            c.indicator(),
            StorageIndicator::Saved,
            "the newer edit is not persisted yet"
        );
        let second = c.next_batch().unwrap();
        c.finish(second.id, ok(&second));
        assert_eq!(c.indicator(), StorageIndicator::Saved);
    }

    #[test]
    fn quota_errors_surface_as_full_and_recover_after_a_successful_retry() {
        let mut c = StorageController::new(small(), "opfs").unwrap();
        c.save(vec![1; 50]);
        let batch = c.next_batch().unwrap();
        assert!(
            c.finish(batch.id, failing(&batch, StorageError::QuotaExceeded)),
            "retry requested"
        );
        assert_eq!(c.indicator(), StorageIndicator::Full);
        assert!(c.has_pending(), "unsaved work is kept for export or retry");

        let retry = c.next_batch().unwrap();
        assert!(!c.finish(retry.id, ok(&retry)));
        assert_eq!(c.indicator(), StorageIndicator::Saved);
    }

    #[test]
    fn a_workspace_over_the_memory_budget_is_refused_and_reported_as_full() {
        let mut c = StorageController::new(small(), "opfs").unwrap();
        c.save(vec![0; 5_000]);
        assert_eq!(c.indicator(), StorageIndicator::Full);
        assert!(c.next_batch().is_none(), "nothing was accepted");
        // A smaller save clears the refusal.
        c.save(vec![0; 100]);
        assert_eq!(c.indicator(), StorageIndicator::Unsaved);
    }

    #[test]
    fn permission_loss_and_missing_backend_are_failures_not_silent_successes() {
        let mut c = StorageController::new(small(), "opfs").unwrap();
        c.save(vec![1; 10]);
        let batch = c.next_batch().unwrap();
        c.finish(batch.id, failing(&batch, StorageError::PermissionDenied));
        assert!(matches!(c.indicator(), StorageIndicator::Failed(_)));

        let none = StorageController::new(small(), "none").unwrap();
        assert!(matches!(none.indicator(), StorageIndicator::Failed(_)));
    }

    #[test]
    fn dom_exception_names_map_to_named_storage_errors() {
        assert_eq!(
            error_from_js("QuotaExceededError", ""),
            StorageError::QuotaExceeded
        );
        assert_eq!(
            error_from_js("NS_ERROR_DOM_QUOTA_REACHED", "x"),
            StorageError::QuotaExceeded
        );
        assert_eq!(
            error_from_js("NotAllowedError", ""),
            StorageError::PermissionDenied
        );
        assert_eq!(
            error_from_js("SecurityError", "blocked"),
            StorageError::PermissionDenied
        );
        assert_eq!(
            error_from_js("DataError", "bad key"),
            StorageError::Backend("DataError: bad key".to_string())
        );
        assert_eq!(
            error_from_js("AbortError", ""),
            StorageError::Backend("AbortError".to_string())
        );
    }

    #[test]
    fn hydrated_data_counts_as_saved_and_estimates_set_the_cold_budget() {
        let mut c = StorageController::new(WORKSPACE_BUDGET, "opfs").unwrap();
        c.apply_estimate(1_000_000, 900_000);
        assert!(c.hydrate(vec![7; 64]).is_ok());
        assert_eq!(c.indicator(), StorageIndicator::Saved);
        c.save(vec![0; 200_000]);
        assert_eq!(
            c.indicator(),
            StorageIndicator::Full,
            "bigger than the remaining quota"
        );
        assert_eq!(c.backend(), "opfs");
        assert_eq!(c.key().as_str(), "drawcraft/web/workspace");
    }
}
