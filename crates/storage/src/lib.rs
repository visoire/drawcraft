//! `drawcraft-storage` (L4) - portable tiered storage model for tabs that cannot hold a large
//! document whole (iPadOS Safari, OPFS / IndexedDB persistence).
//!
//! The crate is a *synchronous mirror over asynchronous persistence*. It performs no I/O: a host
//! (browser glue, desktop file layer, test fake) drains [`TierStore::begin_flush`] batches into
//! the real persistence layer and reports results through [`TierStore::complete_flush`].
//!
//! Guarantees (modelled on the RFC-0023 acceptance requirements):
//! * **Two-step "flushed".** [`TierStore::put`] only *accepts* a write into the hot mirror.
//!   Only a successful persistence acknowledgment marks a revision saved.
//! * **Revision-bound acknowledgments.** An acknowledgment saves exactly the revision it
//!   persisted; a late acknowledgment for an older revision never marks a newer edit saved.
//! * **Failures retry.** Failed, partial or abandoned flushes return writes to the pending queue
//!   with per-key results (quota, permission loss, worker restart).
//! * **Bounded memory.** Hot mirror, pending writes and in-flight copies each have a byte cap;
//!   exceeding one yields [`StorageError::Backpressure`] or [`StorageError::TooLarge`], never
//!   silent growth.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

/// Longest accepted key in bytes.
pub const MAX_KEY_BYTES: usize = 256;
/// Longest accepted key segment (app, document or name) in bytes.
pub const MAX_SEGMENT_BYTES: usize = 96;

/// Why a storage operation was refused or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    /// Key segments must be 1..=96 bytes of `[A-Za-z0-9._-]` and not `.` or `..`.
    InvalidKey(String),
    /// A single value can never fit within the configured budget.
    TooLarge { len: usize, cap: usize },
    /// A budget is exhausted right now; flush or free memory and retry.
    Backpressure { pending_bytes: usize, cap: usize },
    /// The persistence layer is out of space (`QuotaExceededError`).
    QuotaExceeded,
    /// The persistence layer refused access (revoked permission).
    PermissionDenied,
    /// The persistence worker restarted before acknowledging.
    WorkerRestarted,
    /// A server ignored a range request and returned more than was asked for.
    RangeIgnored { requested: u64, received: u64 },
    /// The write was based on an older revision than the current one (strict writer policy).
    StaleBase { current: Revision },
    /// Any other persistence failure reported by the host.
    Backend(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey(why) => write!(f, "invalid storage key: {why}"),
            Self::TooLarge { len, cap } => {
                write!(f, "value of {len} bytes exceeds the {cap} byte budget")
            }
            Self::Backpressure { pending_bytes, cap } => {
                write!(
                    f,
                    "storage backpressure: {pending_bytes} of {cap} pending bytes in use"
                )
            }
            Self::QuotaExceeded => write!(f, "storage quota exceeded"),
            Self::PermissionDenied => write!(f, "storage permission denied"),
            Self::WorkerRestarted => write!(f, "storage worker restarted before acknowledging"),
            Self::RangeIgnored {
                requested,
                received,
            } => {
                write!(
                    f,
                    "range request for {requested} bytes returned {received} bytes"
                )
            }
            Self::StaleBase { current } => {
                write!(f, "write based on a stale revision (current {})", current.0)
            }
            Self::Backend(msg) => write!(f, "storage backend error: {msg}"),
        }
    }
}

impl std::error::Error for StorageError {}

/// Namespaced storage key: `app/document/name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorageKey(String);

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SEGMENT_BYTES
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl StorageKey {
    /// Builds a key, keeping apps and documents in separate namespaces.
    pub fn new(app: &str, document: &str, name: &str) -> Result<Self, StorageError> {
        for (label, seg) in [("app", app), ("document", document), ("name", name)] {
            if !valid_segment(seg) {
                return Err(StorageError::InvalidKey(format!("bad {label} segment")));
            }
        }
        let key = format!("{app}/{document}/{name}");
        if key.len() > MAX_KEY_BYTES {
            return Err(StorageError::InvalidKey("key too long".to_string()));
        }
        Ok(Self(key))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Monotonic write revision. A larger value is always a newer edit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub u64);

/// Identifier of one flush batch handed to the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FlushId(pub u64);

/// Byte budgets. Every cap counts resident bytes, never a claim about total device memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Hot mirror (resident values, clean and dirty).
    pub hot_bytes: usize,
    /// Accepted-but-unpersisted writes.
    pub pending_bytes: usize,
    /// Bytes handed to the host in flush batches and not yet acknowledged.
    pub in_flight_bytes: usize,
    /// Persistence capacity reported by the platform (`navigator.storage.estimate()` remaining),
    /// if known.
    pub cold_bytes: Option<usize>,
}

impl Budget {
    /// Conservative defaults for an iPadOS Safari tab.
    #[must_use]
    pub const fn tablet_default() -> Self {
        Self {
            hot_bytes: 64 * 1024 * 1024,
            pending_bytes: 32 * 1024 * 1024,
            in_flight_bytes: 16 * 1024 * 1024,
            cold_bytes: None,
        }
    }

    /// Records the platform's remaining persistence capacity (`quota - usage`).
    #[must_use]
    pub fn with_storage_estimate(mut self, quota: u64, usage: u64) -> Self {
        let remaining = quota.saturating_sub(usage);
        self.cold_bytes = Some(usize::try_from(remaining).unwrap_or(usize::MAX));
        self
    }

    /// Upper bound on bytes this store keeps in memory.
    #[must_use]
    pub fn memory_cap(&self) -> usize {
        self.hot_bytes.saturating_add(self.in_flight_bytes)
    }
}

/// Persistence state of one key as shown to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStatus {
    /// Never stored.
    NotStored,
    /// Accepted into the hot mirror; not yet handed to the host.
    Unsaved { revision: Revision },
    /// Handed to the host; awaiting acknowledgment.
    Saving { revision: Revision },
    /// Acknowledged as persisted. Only this state may be shown as "Saved".
    Saved { revision: Revision },
}

/// How concurrent writers to one key are treated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WriterPolicy {
    /// A single owner (shared worker) serializes all writes: always accept.
    #[default]
    SingleOwner,
    /// Dedicated-worker fallback: last writer wins, but only against the revision the writer saw.
    LastWriterWinsPerRevision,
}

/// One write handed to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlushWrite {
    pub key: StorageKey,
    pub revision: Revision,
    pub data: Arc<[u8]>,
}

/// A batch of writes for the host to persist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlushBatch {
    pub id: FlushId,
    pub writes: Vec<FlushWrite>,
}

/// Host-reported result for one write of a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlushOutcome {
    pub key: StorageKey,
    pub revision: Revision,
    pub result: Result<(), StorageError>,
}

/// Per-key outcome of acknowledging a batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlushReport {
    /// Revisions now durably saved.
    pub saved: Vec<(StorageKey, Revision)>,
    /// Persisted, but a newer edit is already pending, so the key is not "saved" yet.
    pub superseded: Vec<(StorageKey, Revision)>,
    /// Acknowledgments older than what is already persisted; ignored.
    pub stale: Vec<(StorageKey, Revision)>,
    /// Failed writes, restored to the pending queue for retry.
    pub retry: Vec<(StorageKey, Revision, StorageError)>,
}

/// Memory accounting snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StorageStats {
    pub hot_bytes: usize,
    pub hot_entries: usize,
    pub pending_bytes: usize,
    pub in_flight_bytes: usize,
    pub persisted_bytes: usize,
}

struct Entry {
    data: Arc<[u8]>,
    tick: u64,
    dirty: bool,
}

struct Pending {
    revision: Revision,
    data: Arc<[u8]>,
    in_flight: Option<FlushId>,
}

/// Hot-tier LRU mirror with pending-write tracking.
pub struct TierStore {
    budget: Budget,
    policy: WriterPolicy,
    entries: HashMap<StorageKey, Entry>,
    lru: BTreeMap<u64, StorageKey>,
    pending: BTreeMap<StorageKey, Pending>,
    persisted: HashMap<StorageKey, (Revision, usize)>,
    hot_bytes: usize,
    pending_bytes: usize,
    persisted_bytes: usize,
    tick: u64,
    next_revision: u64,
    next_flush: u64,
}

impl TierStore {
    #[must_use]
    pub fn new(budget: Budget) -> Self {
        Self::with_policy(budget, WriterPolicy::default())
    }

    #[must_use]
    pub fn with_policy(budget: Budget, policy: WriterPolicy) -> Self {
        Self {
            budget,
            policy,
            entries: HashMap::new(),
            lru: BTreeMap::new(),
            pending: BTreeMap::new(),
            persisted: HashMap::new(),
            hot_bytes: 0,
            pending_bytes: 0,
            persisted_bytes: 0,
            tick: 0,
            next_revision: 1,
            next_flush: 1,
        }
    }

    #[must_use]
    pub fn budget(&self) -> Budget {
        self.budget
    }

    #[must_use]
    pub fn stats(&self) -> StorageStats {
        StorageStats {
            hot_bytes: self.hot_bytes,
            hot_entries: self.entries.len(),
            pending_bytes: self.pending_bytes,
            in_flight_bytes: self.in_flight_bytes(),
            persisted_bytes: self.persisted_bytes,
        }
    }

    fn in_flight_bytes(&self) -> usize {
        self.pending
            .values()
            .filter(|p| p.in_flight.is_some())
            .map(|p| p.data.len())
            .sum()
    }

    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.saturating_add(1);
        self.tick
    }

    fn current_revision(&self, key: &StorageKey) -> Revision {
        let pending = self.pending.get(key).map(|p| p.revision);
        let persisted = self.persisted.get(key).map(|(r, _)| *r);
        pending.max(persisted).unwrap_or_default()
    }

    /// Step one of a save: accepts `data` into the hot mirror and the pending queue.
    ///
    /// Returns the revision the write captured. The key is *not* saved until a matching
    /// acknowledgment arrives through [`Self::complete_flush`].
    pub fn put(&mut self, key: &StorageKey, data: Vec<u8>) -> Result<Revision, StorageError> {
        self.put_inner(key, data, None)
    }

    /// Like [`Self::put`] but, under [`WriterPolicy::LastWriterWinsPerRevision`], refuses a write
    /// based on a revision older than the current one.
    pub fn put_checked(
        &mut self,
        key: &StorageKey,
        data: Vec<u8>,
        base: Revision,
    ) -> Result<Revision, StorageError> {
        self.put_inner(key, data, Some(base))
    }

    fn put_inner(
        &mut self,
        key: &StorageKey,
        data: Vec<u8>,
        base: Option<Revision>,
    ) -> Result<Revision, StorageError> {
        let len = data.len();
        let caps = [
            self.budget.hot_bytes,
            self.budget.pending_bytes,
            self.budget.in_flight_bytes,
        ];
        if let Some(cap) = caps.into_iter().min().filter(|cap| len > *cap) {
            return Err(StorageError::TooLarge { len, cap });
        }
        if self.policy == WriterPolicy::LastWriterWinsPerRevision
            && let Some(base) = base
        {
            let current = self.current_revision(key);
            if base < current {
                return Err(StorageError::StaleBase { current });
            }
        }
        let old_pending = self.pending.get(key).map_or(0, |p| p.data.len());
        let projected_pending = self
            .pending_bytes
            .saturating_sub(old_pending)
            .saturating_add(len);
        if projected_pending > self.budget.pending_bytes {
            return Err(StorageError::Backpressure {
                pending_bytes: self.pending_bytes,
                cap: self.budget.pending_bytes,
            });
        }
        if let Some(cold) = self.budget.cold_bytes {
            let old_persisted = self.persisted.get(key).map_or(0, |(_, l)| *l);
            let projected_cold = self
                .persisted_bytes
                .saturating_sub(old_persisted)
                .saturating_add(projected_pending);
            if projected_cold > cold {
                return Err(StorageError::QuotaExceeded);
            }
        }
        // Drop the old mirror entry first so replacing a key never needs double room.
        self.remove_entry(key);
        self.ensure_hot_room(len)?;

        let data: Arc<[u8]> = Arc::from(data);
        let revision = Revision(self.next_revision);
        self.next_revision = self.next_revision.saturating_add(1);
        self.pending_bytes = projected_pending;
        self.pending.insert(
            key.clone(),
            Pending {
                revision,
                data: Arc::clone(&data),
                in_flight: None,
            },
        );
        self.insert_entry(key.clone(), data, true);
        Ok(revision)
    }

    fn remove_entry(&mut self, key: &StorageKey) {
        if let Some(old) = self.entries.remove(key) {
            self.lru.remove(&old.tick);
            self.hot_bytes = self.hot_bytes.saturating_sub(old.data.len());
        }
    }

    fn insert_entry(&mut self, key: StorageKey, data: Arc<[u8]>, dirty: bool) {
        let tick = self.next_tick();
        self.hot_bytes = self.hot_bytes.saturating_add(data.len());
        self.lru.insert(tick, key.clone());
        self.entries.insert(key, Entry { data, tick, dirty });
    }

    /// Evicts least-recently-used *clean* entries until `len` more bytes fit.
    fn ensure_hot_room(&mut self, len: usize) -> Result<(), StorageError> {
        while self.hot_bytes.saturating_add(len) > self.budget.hot_bytes {
            let victim = self
                .lru
                .iter()
                .find(|(_, k)| self.entries.get(*k).is_some_and(|e| !e.dirty))
                .map(|(tick, k)| (*tick, k.clone()));
            let Some((tick, key)) = victim else {
                return Err(StorageError::Backpressure {
                    pending_bytes: self.pending_bytes,
                    cap: self.budget.hot_bytes,
                });
            };
            self.lru.remove(&tick);
            if let Some(e) = self.entries.remove(&key) {
                self.hot_bytes = self.hot_bytes.saturating_sub(e.data.len());
            }
        }
        Ok(())
    }

    /// Reads from the hot mirror (refreshing its LRU position). `None` means the host must
    /// hydrate the key from persistence.
    pub fn get(&mut self, key: &StorageKey) -> Option<Arc<[u8]>> {
        let tick = self.next_tick();
        let entry = self.entries.get_mut(key)?;
        self.lru.remove(&entry.tick);
        entry.tick = tick;
        self.lru.insert(tick, key.clone());
        Some(Arc::clone(&entry.data))
    }

    /// Inserts a value loaded from persistence as a clean entry. Returns `Ok(false)` when a
    /// newer pending edit exists, so a late hydrate can never overwrite it.
    pub fn hydrate(&mut self, key: &StorageKey, data: Vec<u8>) -> Result<bool, StorageError> {
        if self.pending.contains_key(key) {
            return Ok(false);
        }
        let len = data.len();
        if len > self.budget.hot_bytes {
            return Err(StorageError::TooLarge {
                len,
                cap: self.budget.hot_bytes,
            });
        }
        self.remove_entry(key);
        self.ensure_hot_room(len)?;
        self.insert_entry(key.clone(), Arc::from(data), false);
        self.persisted
            .entry(key.clone())
            .or_insert_with(|| (Revision::default(), len));
        Ok(true)
    }

    /// Whole-file fallback: refuses a file whose `stat` size exceeds the hot budget before any
    /// byte is buffered.
    pub fn admit_whole_read(&self, stat_len: u64) -> Result<(), StorageError> {
        let cap = self.budget.hot_bytes;
        match usize::try_from(stat_len) {
            Ok(len) if len <= cap => Ok(()),
            _ => Err(StorageError::TooLarge {
                len: usize::try_from(stat_len).unwrap_or(usize::MAX),
                cap,
            }),
        }
    }

    /// Validates a ranged response: a server that ignored the range and returned everything is
    /// refused when it exceeds what was asked for or the hot budget.
    pub fn check_range_response(&self, requested: u64, received: u64) -> Result<(), StorageError> {
        let cap = u64::try_from(self.budget.hot_bytes).unwrap_or(u64::MAX);
        if received > requested || received > cap {
            return Err(StorageError::RangeIgnored {
                requested,
                received,
            });
        }
        Ok(())
    }

    /// Hands pending writes to the host, oldest key first, within the in-flight byte budget.
    /// Returns `None` when nothing is ready (either empty, or all in flight / over budget).
    pub fn begin_flush(&mut self) -> Option<FlushBatch> {
        let id = FlushId(self.next_flush);
        let mut in_flight = self.in_flight_bytes();
        let mut writes = Vec::new();
        for (key, pending) in &mut self.pending {
            if pending.in_flight.is_some() {
                continue;
            }
            let len = pending.data.len();
            if in_flight.saturating_add(len) > self.budget.in_flight_bytes {
                break;
            }
            in_flight = in_flight.saturating_add(len);
            pending.in_flight = Some(id);
            writes.push(FlushWrite {
                key: key.clone(),
                revision: pending.revision,
                data: Arc::clone(&pending.data),
            });
        }
        if writes.is_empty() {
            return None;
        }
        self.next_flush = self.next_flush.saturating_add(1);
        Some(FlushBatch { id, writes })
    }

    /// Step two of a save: applies host-reported results for batch `id`.
    ///
    /// Each success saves exactly the revision it persisted. Writes missing from `outcomes`
    /// (a partial flush) and failed writes go back to the pending queue.
    pub fn complete_flush(&mut self, id: FlushId, outcomes: Vec<FlushOutcome>) -> FlushReport {
        let mut report = FlushReport::default();
        for outcome in outcomes {
            let FlushOutcome {
                key,
                revision,
                result,
            } = outcome;
            match result {
                Ok(()) => self.apply_success(id, key, revision, &mut report),
                Err(err) => {
                    if let Some(p) = self.pending.get_mut(&key)
                        && p.in_flight == Some(id)
                        && p.revision == revision
                    {
                        p.in_flight = None;
                    }
                    report.retry.push((key, revision, err));
                }
            }
        }
        // Anything still marked in flight for this batch was not acknowledged: retry it.
        let missing: Vec<(StorageKey, Revision)> = self
            .pending
            .iter()
            .filter(|(_, p)| p.in_flight == Some(id))
            .map(|(k, p)| (k.clone(), p.revision))
            .collect();
        for (key, revision) in missing {
            if let Some(p) = self.pending.get_mut(&key) {
                p.in_flight = None;
            }
            report.retry.push((
                key,
                revision,
                StorageError::Backend("no result reported for write".to_string()),
            ));
        }
        report
    }

    fn apply_success(
        &mut self,
        id: FlushId,
        key: StorageKey,
        revision: Revision,
        report: &mut FlushReport,
    ) {
        let persisted_rev = self.persisted.get(&key).map(|(r, _)| *r);
        if persisted_rev.is_some_and(|p| p > revision) {
            report.stale.push((key, revision));
            return;
        }
        // Record durable persistence of exactly this revision's bytes.
        let len = self
            .pending
            .get(&key)
            .filter(|p| p.revision == revision)
            .map(|p| p.data.len())
            .or_else(|| {
                // Older acknowledgment: size of what was written is not retained; keep the
                // previous accounting rather than guessing.
                self.persisted.get(&key).map(|(_, l)| *l)
            })
            .unwrap_or(0);
        let old_len = self.persisted.get(&key).map_or(0, |(_, l)| *l);
        self.persisted.insert(key.clone(), (revision, len));
        self.persisted_bytes = self
            .persisted_bytes
            .saturating_sub(old_len)
            .saturating_add(len);

        match self.pending.get_mut(&key) {
            Some(p) if p.revision == revision => {
                self.pending_bytes = self.pending_bytes.saturating_sub(p.data.len());
                self.pending.remove(&key);
                if let Some(e) = self.entries.get_mut(&key) {
                    e.dirty = false;
                }
                report.saved.push((key, revision));
            }
            Some(p) => {
                // A newer edit is pending; this acknowledgment must not mark it saved.
                if p.in_flight == Some(id) {
                    p.in_flight = None;
                }
                report.superseded.push((key, revision));
            }
            None => report.saved.push((key, revision)),
        }
    }

    /// The persistence worker restarted: every in-flight batch is abandoned and its writes are
    /// queued again. Returns how many writes were restored.
    pub fn worker_restarted(&mut self) -> usize {
        let mut restored = 0;
        for p in self.pending.values_mut() {
            if p.in_flight.take().is_some() {
                restored += 1;
            }
        }
        restored
    }

    /// Persistence state of `key`.
    #[must_use]
    pub fn status(&self, key: &StorageKey) -> SaveStatus {
        if let Some(p) = self.pending.get(key) {
            return if p.in_flight.is_some() {
                SaveStatus::Saving {
                    revision: p.revision,
                }
            } else {
                SaveStatus::Unsaved {
                    revision: p.revision,
                }
            };
        }
        match self.persisted.get(key) {
            Some((revision, _)) => SaveStatus::Saved {
                revision: *revision,
            },
            None => SaveStatus::NotStored,
        }
    }

    /// `true` when nothing is waiting to be persisted.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.pending.is_empty()
    }

    /// Frees every clean entry (memory-pressure response). Dirty entries are kept.
    pub fn evict_clean(&mut self) -> usize {
        let clean: Vec<StorageKey> = self
            .entries
            .iter()
            .filter(|(_, e)| !e.dirty)
            .map(|(k, _)| k.clone())
            .collect();
        let count = clean.len();
        for key in clean {
            self.remove_entry(&key);
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> StorageKey {
        StorageKey::new("drawcraft", "nb-1", name).unwrap_or_else(|_| {
            // Test keys are static and valid; fall back keeps the helper panic-free.
            StorageKey("drawcraft/nb-1/fallback".to_string())
        })
    }

    fn small() -> Budget {
        Budget {
            hot_bytes: 100,
            pending_bytes: 60,
            in_flight_bytes: 60,
            cold_bytes: None,
        }
    }

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

    #[test]
    fn accepted_is_not_saved_until_the_host_acknowledges() -> Result<(), StorageError> {
        let mut s = TierStore::new(small());
        let k = key("page-1");
        let rev = s.put(&k, vec![1; 10])?;
        assert_eq!(s.status(&k), SaveStatus::Unsaved { revision: rev });
        let batch = s
            .begin_flush()
            .ok_or(StorageError::Backend("empty".into()))?;
        assert_eq!(s.status(&k), SaveStatus::Saving { revision: rev });
        let report = s.complete_flush(batch.id, ok(&batch));
        assert_eq!(report.saved, vec![(k.clone(), rev)]);
        assert_eq!(s.status(&k), SaveStatus::Saved { revision: rev });
        assert!(s.is_clean());
        assert_eq!(s.stats().pending_bytes, 0);
        Ok(())
    }

    #[test]
    fn a_late_ack_for_an_older_revision_never_marks_a_newer_edit_saved() -> Result<(), StorageError>
    {
        let mut s = TierStore::new(small());
        let k = key("page-1");
        let r1 = s.put(&k, vec![1; 10])?;
        let first = s
            .begin_flush()
            .ok_or(StorageError::Backend("empty".into()))?;
        let r2 = s.put(&k, vec![2; 12])?;
        assert!(r2 > r1);

        let report = s.complete_flush(first.id, ok(&first));
        assert_eq!(report.superseded, vec![(k.clone(), r1)]);
        assert!(report.saved.is_empty());
        assert_eq!(s.status(&k), SaveStatus::Unsaved { revision: r2 });
        assert_eq!(s.get(&k).map(|d| d.to_vec()), Some(vec![2; 12]));

        let second = s
            .begin_flush()
            .ok_or(StorageError::Backend("empty".into()))?;
        assert_eq!(second.writes[0].revision, r2);
        s.complete_flush(second.id, ok(&second));
        assert_eq!(s.status(&k), SaveStatus::Saved { revision: r2 });
        Ok(())
    }

    #[test]
    fn stale_acks_are_ignored() -> Result<(), StorageError> {
        let mut s = TierStore::new(small());
        let k = key("p");
        let r1 = s.put(&k, vec![1; 5])?;
        let b1 = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
        let r2 = s.put(&k, vec![2; 5])?;
        let b2 = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
        s.complete_flush(b2.id, ok(&b2));
        // The old batch's acknowledgment arrives after the newer one.
        let report = s.complete_flush(b1.id, ok(&b1));
        assert_eq!(report.stale, vec![(k.clone(), r1)]);
        assert_eq!(s.status(&k), SaveStatus::Saved { revision: r2 });
        Ok(())
    }

    #[test]
    fn named_failures_restore_writes_for_retry_with_per_key_results() -> Result<(), StorageError> {
        for failure in [
            StorageError::QuotaExceeded,
            StorageError::PermissionDenied,
            StorageError::WorkerRestarted,
        ] {
            let mut s = TierStore::new(small());
            let (a, b) = (key("a"), key("b"));
            let ra = s.put(&a, vec![1; 10])?;
            let rb = s.put(&b, vec![2; 10])?;
            let batch = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
            let report = s.complete_flush(
                batch.id,
                vec![
                    FlushOutcome {
                        key: a.clone(),
                        revision: ra,
                        result: Ok(()),
                    },
                    FlushOutcome {
                        key: b.clone(),
                        revision: rb,
                        result: Err(failure.clone()),
                    },
                ],
            );
            assert_eq!(report.saved, vec![(a.clone(), ra)]);
            assert_eq!(report.retry, vec![(b.clone(), rb, failure)]);
            assert_eq!(s.status(&b), SaveStatus::Unsaved { revision: rb });

            let retry = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
            assert_eq!(retry.writes.len(), 1);
            s.complete_flush(retry.id, ok(&retry));
            assert_eq!(s.status(&b), SaveStatus::Saved { revision: rb });
        }
        Ok(())
    }

    #[test]
    fn partial_flush_and_worker_restart_requeue_unacknowledged_writes() -> Result<(), StorageError>
    {
        let mut s = TierStore::new(small());
        let (a, b) = (key("a"), key("b"));
        let ra = s.put(&a, vec![1; 10])?;
        s.put(&b, vec![2; 10])?;
        let batch = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
        let report = s.complete_flush(
            batch.id,
            vec![FlushOutcome {
                key: a.clone(),
                revision: ra,
                result: Ok(()),
            }],
        );
        assert_eq!(report.retry.len(), 1);
        assert_eq!(report.retry[0].0, b);
        assert_eq!(
            s.status(&b),
            SaveStatus::Unsaved {
                revision: report.retry[0].1
            }
        );

        let again = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
        assert_eq!(
            s.status(&b),
            SaveStatus::Saving {
                revision: again.writes[0].revision
            }
        );
        assert_eq!(s.worker_restarted(), 1);
        assert_eq!(
            s.status(&b),
            SaveStatus::Unsaved {
                revision: again.writes[0].revision
            }
        );
        assert!(
            s.begin_flush().is_some(),
            "restored write can be flushed again"
        );
        Ok(())
    }

    #[test]
    fn budgets_refuse_instead_of_growing_silently() -> Result<(), StorageError> {
        let mut s = TierStore::new(small());
        assert_eq!(
            s.put(&key("huge"), vec![0; 61]),
            Err(StorageError::TooLarge { len: 61, cap: 60 })
        );
        s.put(&key("a"), vec![0; 30])?;
        s.put(&key("b"), vec![0; 30])?;
        let refused = s.put(&key("c"), vec![0; 1]);
        assert!(
            matches!(refused, Err(StorageError::Backpressure { .. })),
            "{refused:?}"
        );
        let st = s.stats();
        assert!(st.pending_bytes <= small().pending_bytes);
        assert!(st.hot_bytes <= small().hot_bytes);
        assert!(st.hot_bytes + st.in_flight_bytes <= small().memory_cap());

        // After a successful flush the budget frees up again.
        let batch = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
        s.complete_flush(batch.id, ok(&batch));
        s.put(&key("c"), vec![0; 20])?;
        Ok(())
    }

    #[test]
    fn dirty_entries_are_never_evicted_and_clean_ones_leave_in_lru_order()
    -> Result<(), StorageError> {
        let mut s = TierStore::new(Budget {
            hot_bytes: 40,
            pending_bytes: 40,
            in_flight_bytes: 40,
            cold_bytes: None,
        });
        let (a, b, c) = (key("a"), key("b"), key("c"));
        for k in [&a, &b] {
            s.put(k, vec![0; 20])?;
        }
        let batch = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
        s.complete_flush(batch.id, ok(&batch));
        s.get(&a); // `b` is now least recently used.
        s.put(&c, vec![0; 20])?;
        assert!(s.get(&b).is_none(), "LRU clean entry evicted");
        assert!(s.get(&a).is_some() && s.get(&c).is_some());

        // `c` is dirty and `a` clean: filling hot memory evicts `a`, never `c`.
        s.put(&key("d"), vec![0; 20])?;
        assert!(s.get(&a).is_none());
        assert!(s.get(&c).is_some());
        assert!(matches!(
            s.put(&key("e"), vec![0; 20]),
            Err(StorageError::Backpressure { .. })
        ));
        Ok(())
    }

    #[test]
    fn hydrate_cannot_clobber_a_newer_pending_edit() -> Result<(), StorageError> {
        let mut s = TierStore::new(small());
        let k = key("page");
        s.put(&k, vec![9; 8])?;
        assert_eq!(s.hydrate(&k, vec![1; 8]), Ok(false));
        assert_eq!(s.get(&k).map(|d| d.to_vec()), Some(vec![9; 8]));
        assert_eq!(s.hydrate(&key("other"), vec![3; 8]), Ok(true));
        assert!(matches!(
            s.hydrate(&key("too-big"), vec![0; 101]),
            Err(StorageError::TooLarge { .. })
        ));
        Ok(())
    }

    #[test]
    fn whole_read_fallback_checks_size_and_ignored_ranges() {
        let s = TierStore::new(small());
        assert!(s.admit_whole_read(100).is_ok());
        assert!(matches!(
            s.admit_whole_read(101),
            Err(StorageError::TooLarge { .. })
        ));
        assert!(matches!(
            s.admit_whole_read(u64::MAX),
            Err(StorageError::TooLarge { .. })
        ));
        assert!(s.check_range_response(50, 50).is_ok());
        assert!(matches!(
            s.check_range_response(50, 51),
            Err(StorageError::RangeIgnored { .. })
        ));
        assert!(matches!(
            s.check_range_response(u64::MAX, 101),
            Err(StorageError::RangeIgnored { .. })
        ));
    }

    #[test]
    fn platform_quota_estimate_refuses_writes_the_cold_tier_cannot_hold() -> Result<(), StorageError>
    {
        let budget = small().with_storage_estimate(1_000, 975);
        assert_eq!(budget.cold_bytes, Some(25));
        let mut s = TierStore::new(budget);
        s.put(&key("a"), vec![0; 20])?;
        assert_eq!(
            s.put(&key("b"), vec![0; 10]),
            Err(StorageError::QuotaExceeded)
        );
        assert_eq!(
            Budget::tablet_default()
                .with_storage_estimate(1, 5)
                .cold_bytes,
            Some(0)
        );
        Ok(())
    }

    #[test]
    fn acknowledged_saves_survive_close_and_reopen() -> Result<(), StorageError> {
        let mut disk: HashMap<StorageKey, Vec<u8>> = HashMap::new();
        let mut first = TierStore::new(small());
        let k = key("page-7");
        first.put(&k, b"ink".to_vec())?;
        let batch = first
            .begin_flush()
            .ok_or(StorageError::Backend("e".into()))?;
        for w in &batch.writes {
            disk.insert(w.key.clone(), w.data.to_vec());
        }
        first.complete_flush(batch.id, ok(&batch));
        drop(first);

        let mut reopened = TierStore::new(small());
        assert!(reopened.get(&k).is_none());
        let bytes = disk
            .get(&k)
            .cloned()
            .ok_or(StorageError::Backend("lost".into()))?;
        reopened.hydrate(&k, bytes)?;
        assert_eq!(reopened.get(&k).map(|d| d.to_vec()), Some(b"ink".to_vec()));
        assert!(matches!(reopened.status(&k), SaveStatus::Saved { .. }));
        Ok(())
    }

    #[test]
    fn strict_writer_policy_rejects_stale_bases() -> Result<(), StorageError> {
        let mut s = TierStore::with_policy(small(), WriterPolicy::LastWriterWinsPerRevision);
        let k = key("doc");
        let r1 = s.put(&k, vec![1; 4])?;
        let r2 = s.put_checked(&k, vec![2; 4], r1)?;
        assert_eq!(
            s.put_checked(&k, vec![3; 4], r1),
            Err(StorageError::StaleBase { current: r2 })
        );
        // The single-owner policy serializes writers itself and never refuses on revision.
        let mut owner = TierStore::new(small());
        owner.put(&k, vec![1; 4])?;
        assert!(owner.put_checked(&k, vec![2; 4], Revision(0)).is_ok());
        Ok(())
    }

    #[test]
    fn hostile_keys_are_rejected_and_namespaces_are_isolated() -> Result<(), StorageError> {
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a b",
            "ünï",
            "x\0y",
            &"z".repeat(MAX_SEGMENT_BYTES + 1),
        ] {
            assert!(StorageKey::new(bad, "d", "n").is_err(), "{bad:?}");
            assert!(StorageKey::new("a", bad, "n").is_err(), "{bad:?}");
            assert!(StorageKey::new("a", "d", bad).is_err(), "{bad:?}");
        }
        let one = StorageKey::new("drawcraft", "nb-1", "page-1")?;
        let other_doc = StorageKey::new("drawcraft", "nb-2", "page-1")?;
        let other_app = StorageKey::new("other-app", "nb-1", "page-1")?;
        assert!(one != other_doc && one != other_app);
        let mut s = TierStore::new(small());
        s.put(&one, vec![1])?;
        assert!(s.get(&other_doc).is_none() && s.get(&other_app).is_none());
        Ok(())
    }

    #[test]
    fn evict_clean_frees_memory_but_keeps_unsaved_work() -> Result<(), StorageError> {
        let mut s = TierStore::new(small());
        s.put(&key("a"), vec![0; 10])?;
        let batch = s.begin_flush().ok_or(StorageError::Backend("e".into()))?;
        s.complete_flush(batch.id, ok(&batch));
        s.put(&key("b"), vec![0; 10])?;
        assert_eq!(s.evict_clean(), 1);
        assert_eq!(s.stats().hot_bytes, 10);
        assert!(s.get(&key("b")).is_some());
        Ok(())
    }
}
