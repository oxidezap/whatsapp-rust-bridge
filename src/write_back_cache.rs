//! Optional write-back cache for the JS storage backend.
//!
//! ## Why this exists
//!
//! The core (`whatsapp-rust`) keeps Signal protocol state in its own in-memory
//! `SignalStoreCache` and calls `backend.put_session(...)` (and friends) on the
//! hot path — once per decrypt and once per encrypt — to *durably* persist the
//! advancing double-ratchet. Each of those calls crosses the JS↔WASM boundary
//! (a `Uint8Array` copy + externref register + a JS `Map.set`). Profiling a
//! pingpong flood showed this per-message state persistence — not message
//! content serialization — dominates the boundary heap traffic (~13 store
//! writes / ~2.3 KB per message).
//!
//! For an **ephemeral** store (the in-memory store used by benchmarks/tests)
//! that per-message durability is wasted: the data is lost on process exit
//! regardless. This cache absorbs the core's per-message writes in WASM memory
//! (a cheap `Vec` copy, no boundary crossing) and only crosses to the JS store
//! on an explicit `flush` (disconnect / logout / shutdown).
//!
//! ## Durability contract
//!
//! Write-back trades durability for fewer crossings: a crash before `flush`
//! loses every un-flushed write. It is therefore **opt-in** via the host's
//! `capabilities.writeBack` and intended ONLY for ephemeral stores. A durable
//! (file/DB) store must leave it off so every write goes straight through.
//!
//! This module is pure Rust (no JS) so the cache logic — read-after-write,
//! tombstones, key-set merge, flush draining — is unit-tested in isolation.

use std::collections::{HashMap, HashSet};

/// A cached value for one `(store, key)`.
enum CacheVal {
    /// Present with these bytes.
    Present(Vec<u8>),
    /// Deleted — a tombstone. Reads must return "absent" instead of falling
    /// through to a stale value still sitting in the JS store; the flush turns
    /// it into a real `delete`.
    Tomb,
}

/// Per-namespace key→value map plus a global dirty set of `(store, key)` pairs
/// pending a flush to the JS backend.
pub(crate) struct WriteBackCache {
    stores: HashMap<String, HashMap<String, CacheVal>>,
    dirty: HashSet<(String, String)>,
}

/// The dirty entries grouped per namespace for a single flush, split into
/// upserts (`sets`) and removals (`deletes`) so the caller can drive the
/// backend's batch `setMany`/`deleteMany` (one crossing per namespace).
pub(crate) struct FlushBatch {
    pub sets: HashMap<String, Vec<(String, Vec<u8>)>>,
    pub deletes: HashMap<String, Vec<String>>,
}

impl FlushBatch {
    /// Used by the unit tests; the runtime path drains only when dirty is
    /// non-empty (`dirty_is_empty` guards `flush_cache`).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty() && self.deletes.is_empty()
    }
}

impl WriteBackCache {
    pub fn new() -> Self {
        Self {
            stores: HashMap::new(),
            dirty: HashSet::new(),
        }
    }

    /// Look a key up.
    /// - `None` — not in the cache; caller must consult the backend.
    /// - `Some(None)` — tombstoned (deleted); caller returns "absent".
    /// - `Some(Some(bytes))` — present with `bytes`.
    pub fn lookup(&self, store: &str, key: &str) -> Option<Option<&[u8]>> {
        match self.stores.get(store)?.get(key)? {
            CacheVal::Present(v) => Some(Some(v.as_slice())),
            CacheVal::Tomb => Some(None),
        }
    }

    /// Record a write (marks the entry dirty for the next flush).
    pub fn write(&mut self, store: &str, key: &str, val: Vec<u8>) {
        self.stores
            .entry(store.to_string())
            .or_default()
            .insert(key.to_string(), CacheVal::Present(val));
        self.dirty.insert((store.to_string(), key.to_string()));
    }

    /// Record a delete as a tombstone (marks dirty).
    pub fn delete(&mut self, store: &str, key: &str) {
        self.stores
            .entry(store.to_string())
            .or_default()
            .insert(key.to_string(), CacheVal::Tomb);
        self.dirty.insert((store.to_string(), key.to_string()));
    }

    /// Populate from a backend read result — **clean** (not dirty) and only when
    /// the key isn't already cached, so a write that raced the backend read (the
    /// read awaits across the FFI boundary) is never clobbered by stale bytes.
    /// `None` caches a negative (absent) lookup as a tombstone so repeat misses
    /// don't re-cross the boundary.
    pub fn populate(&mut self, store: &str, key: &str, val: Option<Vec<u8>>) {
        let ns = self.stores.entry(store.to_string()).or_default();
        if ns.contains_key(key) {
            return; // a concurrent write already owns this key — keep it.
        }
        ns.insert(
            key.to_string(),
            match val {
                Some(v) => CacheVal::Present(v),
                None => CacheVal::Tomb,
            },
        );
        // Intentionally NOT added to `dirty`: this mirrors what the backend
        // already holds, so flushing it would be a redundant write.
    }

    /// Merge cached keys for one namespace into the backend's enumerated key set:
    /// drop tombstoned keys, add present-only keys, then apply the `prefix`
    /// filter. Used so `listKeys` reflects un-flushed writes/deletes.
    pub fn merge_keys(
        &self,
        store: &str,
        backend_keys: Vec<String>,
        prefix: Option<&str>,
    ) -> Vec<String> {
        let mut set: HashSet<String> = backend_keys.into_iter().collect();
        if let Some(ns) = self.stores.get(store) {
            for (k, v) in ns {
                match v {
                    CacheVal::Present(_) => {
                        set.insert(k.clone());
                    }
                    CacheVal::Tomb => {
                        set.remove(k);
                    }
                }
            }
        }
        set.into_iter()
            .filter(|k| prefix.is_none_or(|p| k.starts_with(p)))
            .collect()
    }

    /// Tombstone every present cache key in `store` matching `prefix` (marks
    /// them dirty). Returns the count. Used by `delete_prefix` on hosts that
    /// can't enumerate, so the cache's own matching keys are removed too.
    pub fn tombstone_prefix(&mut self, store: &str, prefix: &str) -> u32 {
        let keys: Vec<String> = match self.stores.get(store) {
            Some(ns) => ns
                .iter()
                .filter(|(k, v)| k.starts_with(prefix) && matches!(v, CacheVal::Present(_)))
                .map(|(k, _)| k.clone())
                .collect(),
            None => return 0,
        };
        for k in &keys {
            self.delete(store, k);
        }
        keys.len() as u32
    }

    /// True when there is nothing pending to flush.
    pub fn dirty_is_empty(&self) -> bool {
        self.dirty.is_empty()
    }

    /// Snapshot every dirty entry into a per-namespace `FlushBatch` WITHOUT
    /// mutating: the dirty set and tombstones are left intact so a *failed*
    /// flush can be retried. Call [`commit_flush`](Self::commit_flush) only
    /// after the backend writes succeed.
    pub fn snapshot_for_flush(&self) -> FlushBatch {
        let mut sets: HashMap<String, Vec<(String, Vec<u8>)>> = HashMap::new();
        let mut deletes: HashMap<String, Vec<String>> = HashMap::new();
        for (store, key) in &self.dirty {
            let Some(ns) = self.stores.get(store) else {
                continue;
            };
            match ns.get(key) {
                Some(CacheVal::Present(v)) => {
                    sets.entry(store.clone())
                        .or_default()
                        .push((key.clone(), v.clone()));
                }
                Some(CacheVal::Tomb) => {
                    deletes.entry(store.clone()).or_default().push(key.clone());
                }
                None => {}
            }
        }
        FlushBatch { sets, deletes }
    }

    /// Mark a successfully-flushed batch clean: drop its keys from the dirty set
    /// and remove flushed tombstones from the cache. Present values stay as clean
    /// cache so subsequent reads still hit. Call ONLY after the backend writes
    /// for `batch` succeeded — on failure, skip this so the entries stay dirty
    /// and are retried on the next flush. (Flush runs at quiescent shutdown
    /// points, so a write racing the flush is not a concern in practice.)
    pub fn commit_flush(&mut self, batch: &FlushBatch) {
        for (store, entries) in &batch.sets {
            for (key, _) in entries {
                self.dirty.remove(&(store.clone(), key.clone()));
            }
        }
        for (store, keys) in &batch.deletes {
            if let Some(ns) = self.stores.get_mut(store) {
                for key in keys {
                    ns.remove(key); // tombstone consumed by the flush
                }
            }
            for key in keys {
                self.dirty.remove(&(store.clone(), key.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Alias `#[test]` -> wasm_bindgen_test so these pure-logic unit tests run on
    // wasm32 via `wasm-pack test --node` (the crate has no native test target).
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn read_after_write_hits_cache() {
        let mut c = WriteBackCache::new();
        c.write("session", "a", vec![1, 2, 3]);
        assert_eq!(c.lookup("session", "a"), Some(Some(&[1, 2, 3][..])));
        // Different namespace, same key → independent.
        assert_eq!(c.lookup("identity", "a"), None);
    }

    #[test]
    fn delete_tombstones_read() {
        let mut c = WriteBackCache::new();
        c.write("session", "a", vec![9]);
        c.delete("session", "a");
        // Tombstone: cached, but reads as absent (NOT a fall-through miss).
        assert_eq!(c.lookup("session", "a"), Some(None));
    }

    #[test]
    fn populate_does_not_clobber_a_pending_write() {
        let mut c = WriteBackCache::new();
        c.write("session", "a", vec![1]); // a write raced ahead
        c.populate("session", "a", Some(vec![2])); // stale backend read
        assert_eq!(c.lookup("session", "a"), Some(Some(&[1][..])));
        // And the write is still dirty (populate must not have touched it).
        assert!(!c.dirty_is_empty());
    }

    #[test]
    fn populate_caches_negative_lookup() {
        let mut c = WriteBackCache::new();
        c.populate("session", "missing", None);
        assert_eq!(c.lookup("session", "missing"), Some(None));
        // Negative population is clean — nothing to flush.
        assert!(c.dirty_is_empty());
    }

    #[test]
    fn merge_keys_adds_present_drops_tombstoned_and_filters_prefix() {
        let mut c = WriteBackCache::new();
        c.write("prekey", "p2", vec![0]); // present, only in cache
        c.delete("prekey", "p1"); // tombstoned (was in backend)
        c.write("prekey", "other", vec![0]); // present, non-matching prefix
        let backend = vec!["p1".to_string(), "p3".to_string()];
        let mut merged = c.merge_keys("prekey", backend, Some("p"));
        merged.sort();
        // p1 dropped (tombstone), p2 added, p3 kept, "other" filtered by prefix.
        assert_eq!(merged, vec!["p2".to_string(), "p3".to_string()]);
    }

    #[test]
    fn snapshot_splits_sets_and_deletes_without_mutating() {
        let mut c = WriteBackCache::new();
        c.write("session", "a", vec![1]);
        c.write("session", "b", vec![2]);
        c.delete("session", "c");
        let batch = c.snapshot_for_flush();
        let mut sets = batch.sets.get("session").cloned().unwrap_or_default();
        sets.sort();
        assert_eq!(
            sets,
            vec![("a".to_string(), vec![1]), ("b".to_string(), vec![2])]
        );
        assert_eq!(
            batch.deletes.get("session").unwrap(),
            &vec!["c".to_string()]
        );
        // Snapshot must NOT mutate — dirty + tombstone still present (retry-safe).
        assert!(!c.dirty_is_empty());
        assert_eq!(c.lookup("session", "c"), Some(None)); // tombstone intact
    }

    #[test]
    fn commit_after_snapshot_clears_dirty_and_consumes_tombstones() {
        let mut c = WriteBackCache::new();
        c.write("session", "a", vec![1]);
        c.delete("session", "c");
        let batch = c.snapshot_for_flush();
        c.commit_flush(&batch);
        // Dirty cleared; present survives as clean cache; tombstone consumed.
        assert!(c.dirty_is_empty());
        assert_eq!(c.lookup("session", "a"), Some(Some(&[1][..])));
        assert_eq!(c.lookup("session", "c"), None);
    }

    #[test]
    fn failed_flush_retains_dirty_for_retry() {
        // Models flush_cache's contract: snapshot, then (write FAILS so) skip
        // commit → the entry stays dirty and is recovered by the next snapshot.
        let mut c = WriteBackCache::new();
        c.write("session", "a", vec![1]);
        let _batch = c.snapshot_for_flush(); // pretend the backend write failed
        // No commit_flush → still dirty, still flushable.
        assert!(!c.dirty_is_empty());
        let retry = c.snapshot_for_flush();
        assert_eq!(
            retry.sets.get("session").unwrap(),
            &vec![("a".to_string(), vec![1])]
        );
    }

    #[test]
    fn snapshot_is_empty_when_nothing_dirty() {
        let mut c = WriteBackCache::new();
        c.populate("session", "a", Some(vec![1])); // clean
        assert!(c.snapshot_for_flush().is_empty());
    }

    #[test]
    fn tombstone_prefix_removes_matching_present_keys() {
        let mut c = WriteBackCache::new();
        c.write("sender_key_devices", "g1:a", vec![1]);
        c.write("sender_key_devices", "g1:b", vec![2]);
        c.write("sender_key_devices", "g2:a", vec![3]);
        let n = c.tombstone_prefix("sender_key_devices", "g1:");
        assert_eq!(n, 2);
        assert_eq!(c.lookup("sender_key_devices", "g1:a"), Some(None));
        assert_eq!(c.lookup("sender_key_devices", "g1:b"), Some(None));
        assert_eq!(c.lookup("sender_key_devices", "g2:a"), Some(Some(&[3][..])));
    }
}
