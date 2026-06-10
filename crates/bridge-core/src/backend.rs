//! Engine-agnostic storage backend.
//!
//! Port of the old wasm `src/js_backend.rs` (+ `write_back_cache.rs` +
//! `js_cache_store.rs`) onto the `Host*` seam. Every storage op that the wasm
//! version performed via `js_sys::Function` now goes through
//! `Arc<dyn HostStore>` / `Arc<dyn HostCacheStore>`, which are already typed
//! (`Vec<u8>` / `String`) and return `anyhow::Result`. The host errors are
//! mapped into `wacore::store::error::StoreError` while keeping a context
//! string, and the STRICT "never silently drop" semantics are preserved by
//! propagating every error.
//!
//! ALL business logic (the 17 store-name constants, composite key formats,
//! 8-byte BE timestamp prefixes, the `meta` JSON index lists, the in-memory
//! `sent_message_keys` cache, the capability-gated enumerate / prefix-delete
//! paths, the `SCAN_CHUNK` TOCTOU expiry sweeps, the write-back cache, and the
//! separate `device/account` prost persistence) is preserved 1:1 with the old
//! wasm code. Cross-references to the original are pinned as `js_backend.rs:NNN`.
//!
//! Differences vs the old code, all forced by the engine-agnostic constraints:
//!   * `async_lock::Mutex` → `tokio::sync::Mutex` (genuinely `Send`, guards
//!     `Send`; the "lock never held across await" invariant is kept).
//!   * `prost::Message` for `Device.account` → `wacore::store::device::
//!     account_serde::{to_bytes,from_bytes}` (identical bytes: literally
//!     `encode_to_vec` / `AdvSignedDeviceIdentity::decode`), avoiding a direct
//!     `prost` dependency.
//!   * `clear_mutation_macs` is a NEW required trait method (no default at this
//!     wacore rev); see its impl for the capability-gated strategy.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::Mutex;

use wacore::appstate::hash::HashState;
use wacore::store::Device;
use wacore::store::InMemoryBackend;
use wacore::store::cache::CacheStore;
use wacore::store::error::Result;
use wacore::store::traits::*;
use wacore_appstate::processor::AppStateMutationMAC;

use crate::host::{HostCacheStore, HostStore};

// ---------------------------------------------------------------------------
// write-back cache (port of write_back_cache.rs — pure Rust, no engine types)
// ---------------------------------------------------------------------------

mod write_back {
    //! Optional write-back cache for the core backend.
    //!
    //! Port of the old wasm `write_back_cache.rs` verbatim (it was already pure
    //! Rust). The core (`whatsapp-rust`) calls `backend.put_session(...)` (and
    //! friends) once per decrypt/encrypt on the hot path to durably persist the
    //! advancing double-ratchet. Each crosses the host store boundary. For an
    //! ephemeral store (benchmarks/tests) that durability is wasted, so this
    //! cache absorbs the per-message writes in-process and only crosses to the
    //! host store on an explicit `flush`.
    //!
    //! Durability contract: a crash before `flush` loses every un-flushed write.
    //! It is therefore opt-in via `capabilities.write_back` and intended ONLY
    //! for ephemeral stores. A durable store must leave it off so every write
    //! goes straight through.

    use std::collections::{HashMap, HashSet};

    /// A cached value for one `(store, key)`.
    enum CacheVal {
        /// Present with these bytes.
        Present(Vec<u8>),
        /// Deleted — a tombstone. Reads must return "absent" instead of falling
        /// through to a stale value still sitting in the host store; the flush
        /// turns it into a real `delete`.
        Tomb,
    }

    /// Per-namespace key→value map plus a global dirty set of `(store, key)`
    /// pairs pending a flush to the host backend.
    pub(super) struct WriteBackCache {
        stores: HashMap<String, HashMap<String, CacheVal>>,
        dirty: HashSet<(String, String)>,
    }

    /// The dirty entries grouped per namespace for a single flush, split into
    /// upserts (`sets`) and removals (`deletes`) so the caller can drive the
    /// backend's batch `set_many`/`delete_many` (one crossing per namespace).
    pub(super) struct FlushBatch {
        pub sets: HashMap<String, Vec<(String, Vec<u8>)>>,
        pub deletes: HashMap<String, Vec<String>>,
    }

    impl FlushBatch {
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

        /// Populate from a backend read result — **clean** (not dirty) and only
        /// when the key isn't already cached, so a write that raced the backend
        /// read is never clobbered by stale bytes. `None` caches a negative
        /// (absent) lookup as a tombstone so repeat misses don't re-cross.
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

        /// Merge cached keys for one namespace into the backend's enumerated key
        /// set: drop tombstoned keys, add present-only keys, then apply the
        /// `prefix` filter. Used so `list_keys` reflects un-flushed writes.
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
        /// them dirty). Returns the count.
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
        /// flush can be retried. Call `commit_flush` only after the backend
        /// writes succeed.
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

        /// Mark a successfully-flushed batch clean: drop its keys from the dirty
        /// set and remove flushed tombstones. Present values stay as clean cache
        /// so subsequent reads still hit. Call ONLY after the backend writes for
        /// `batch` succeeded — on failure, skip this so the entries stay dirty.
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

        #[test]
        fn read_after_write_hits_cache() {
            let mut c = WriteBackCache::new();
            c.write("session", "a", vec![1, 2, 3]);
            assert_eq!(c.lookup("session", "a"), Some(Some(&[1, 2, 3][..])));
            assert_eq!(c.lookup("identity", "a"), None);
        }

        #[test]
        fn delete_tombstones_read() {
            let mut c = WriteBackCache::new();
            c.write("session", "a", vec![9]);
            c.delete("session", "a");
            assert_eq!(c.lookup("session", "a"), Some(None));
        }

        #[test]
        fn populate_does_not_clobber_a_pending_write() {
            let mut c = WriteBackCache::new();
            c.write("session", "a", vec![1]);
            c.populate("session", "a", Some(vec![2]));
            assert_eq!(c.lookup("session", "a"), Some(Some(&[1][..])));
            assert!(!c.dirty_is_empty());
        }

        #[test]
        fn populate_caches_negative_lookup() {
            let mut c = WriteBackCache::new();
            c.populate("session", "missing", None);
            assert_eq!(c.lookup("session", "missing"), Some(None));
            assert!(c.dirty_is_empty());
        }

        #[test]
        fn merge_keys_adds_present_drops_tombstoned_and_filters_prefix() {
            let mut c = WriteBackCache::new();
            c.write("prekey", "p2", vec![0]);
            c.delete("prekey", "p1");
            c.write("prekey", "other", vec![0]);
            let backend = vec!["p1".to_string(), "p3".to_string()];
            let mut merged = c.merge_keys("prekey", backend, Some("p"));
            merged.sort();
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
            assert!(!c.dirty_is_empty());
            assert_eq!(c.lookup("session", "c"), Some(None));
        }

        #[test]
        fn commit_after_snapshot_clears_dirty_and_consumes_tombstones() {
            let mut c = WriteBackCache::new();
            c.write("session", "a", vec![1]);
            c.delete("session", "c");
            let batch = c.snapshot_for_flush();
            c.commit_flush(&batch);
            assert!(c.dirty_is_empty());
            assert_eq!(c.lookup("session", "a"), Some(Some(&[1][..])));
            assert_eq!(c.lookup("session", "c"), None);
        }

        #[test]
        fn failed_flush_retains_dirty_for_retry() {
            let mut c = WriteBackCache::new();
            c.write("session", "a", vec![1]);
            let _batch = c.snapshot_for_flush();
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
            c.populate("session", "a", Some(vec![1]));
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
}

use write_back::WriteBackCache;

// ---------------------------------------------------------------------------
// Store name constants (js_backend.rs:34-50 — 17 stores, 1:1)
// ---------------------------------------------------------------------------

const STORE_IDENTITY: &str = "identity";
const STORE_SESSION: &str = "session";
const STORE_PREKEY: &str = "prekey";
const STORE_SIGNED_PREKEY: &str = "signed_prekey";
const STORE_SENDER_KEY: &str = "sender_key";
const STORE_SYNC_KEY: &str = "sync_key";
const STORE_SYNC_VERSION: &str = "sync_version";
const STORE_MUTATION_MAC: &str = "mutation_mac";
const STORE_DEVICE: &str = "device";
const STORE_SENDER_KEY_DEVICES: &str = "sender_key_devices";
const STORE_LID_MAPPING: &str = "lid_mapping";
const STORE_BASE_KEY: &str = "base_key";
const STORE_DEVICE_LIST: &str = "device_list";
const STORE_TC_TOKEN: &str = "tc_token";
const STORE_SENT_MESSAGE: &str = "sent_message";
const STORE_MSG_SECRET: &str = "msg_secret";
const STORE_META: &str = "meta";

// ---------------------------------------------------------------------------
// Public API: backend factories
// ---------------------------------------------------------------------------

/// Get a new `InMemoryBackend` instance (fallback when no host store is
/// provided). Mirrors `js_backend.rs:57` `new_in_memory_backend`.
pub fn new_in_memory_backend() -> Arc<dyn Backend> {
    Arc::new(InMemoryBackend::default())
}

/// Create a `CoreBackend` over a host store. The batch/enumeration handles are
/// discovered through `HostStore::capabilities()`; when a capability is off the
/// backend falls back to per-key `set`/`delete` and its self-maintained JSON
/// meta-indexes. Mirrors `js_backend.rs:88` `new_js_backend`.
///
/// Returns the concrete `Arc<CoreBackend>` so the caller can both pass it as
/// `Arc<dyn Backend>` to the persistence manager AND keep a handle to call
/// [`CoreBackend::flush_cache`] on disconnect/shutdown.
pub fn new_core_backend(host: Arc<dyn HostStore>) -> Arc<CoreBackend> {
    Arc::new(CoreBackend::new(host))
}

// ---------------------------------------------------------------------------
// CoreBackend struct
// ---------------------------------------------------------------------------

/// Storage backend that delegates all persistence to an `Arc<dyn HostStore>`.
/// Genuinely `Send + Sync` (every field is, and the host trait requires it).
pub struct CoreBackend {
    host: Arc<dyn HostStore>,
    /// True when the host can enumerate a namespace (capabilities.enumerate).
    /// When true the core DROPS its hand-maintained key indexes and derives the
    /// key set from the store directly. (js_backend.rs:112)
    has_enumerate: bool,
    /// True when the host implements delete_prefix (capabilities.prefix_delete).
    has_prefix_delete: bool,
    /// True when the host implements the batch surface (set_many/delete_many/
    /// get_many). The host's optional methods return `false`/`None` when not
    /// actually present, so this only gates the fast path. (js_backend.rs:101)
    has_batch: bool,
    next_device_id: AtomicI32,
    /// In-memory cache of sent message keys — avoids O(n²) JSON re-serialization
    /// on every store_sent_message call. Loaded lazily on first access. Only
    /// used on the self-index path (`!has_enumerate`). (js_backend.rs:119)
    sent_message_keys: Mutex<Option<Vec<String>>>,
    /// Opt-in write-back cache. `Some` when the host declared `write_back`;
    /// every `s_*` helper routes through it so per-message writes stay
    /// in-process and only cross to the host on `flush_cache`. `None` =>
    /// write-through (the default). (js_backend.rs:123)
    write_back: Option<Mutex<WriteBackCache>>,
}

/// Chunk size for value-scanning enumeration (delete-expired sweeps). Bounds
/// how many values are materialized across the host boundary at once so a
/// 20k-entry namespace doesn't spike host-side memory in a single call.
/// (js_backend.rs:131)
const SCAN_CHUNK: usize = 512;

impl CoreBackend {
    fn new(host: Arc<dyn HostStore>) -> Self {
        // A capability is only honored when the host actually advertises it; the
        // optional `HostStore` methods themselves degrade safely (Ok(false) /
        // Ok(None)) when absent, so a misdeclared host can't call a missing fn.
        let caps = host.capabilities();
        Self {
            host,
            has_enumerate: caps.enumerate,
            has_prefix_delete: caps.prefix_delete,
            has_batch: caps.batch,
            next_device_id: AtomicI32::new(1),
            sent_message_keys: Mutex::new(None),
            write_back: if caps.write_back {
                Some(Mutex::new(WriteBackCache::new()))
            } else {
                None
            },
        }
    }

    /// Get or lazily load the sent message keys list. (js_backend.rs:162)
    async fn get_sent_keys(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Vec<String>>>> {
        let mut guard = self.sent_message_keys.lock().await;
        if guard.is_none() {
            let keys: Vec<String> = self
                .get_json(STORE_META, "sent_message_keys")
                .await?
                .unwrap_or_default();
            *guard = Some(keys);
        }
        Ok(guard)
    }

    /// Persist the in-memory key list to the host store. Only called during
    /// cleanup/expiration — never on the send hot path. (js_backend.rs:176)
    async fn flush_sent_keys(&self, keys: &Vec<String>) -> Result<()> {
        self.set_json(STORE_META, "sent_message_keys", keys).await
    }

    // ── Write-back-aware entry points ─────────────────────────────────────
    //
    // Every trait method goes through these. When write-back is OFF (`None`)
    // they delegate straight to the `*_raw` host calls (the original behavior).
    // When ON they serve reads from / absorb writes into the in-process cache,
    // so the host boundary is crossed only by misses and by `flush_cache`.
    // INVARIANT: a cache lock is never held across a host `await`.

    async fn s_get(&self, store: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let Some(wb) = &self.write_back else {
            return self.get_raw(store, key).await;
        };
        {
            let cache = wb.lock().await;
            if let Some(hit) = cache.lookup(store, key) {
                return Ok(hit.map(|b| b.to_vec()));
            }
        }
        let fetched = self.get_raw(store, key).await?;
        let mut cache = wb.lock().await;
        // A write may have landed during the await — it wins over the read.
        if let Some(hit) = cache.lookup(store, key) {
            return Ok(hit.map(|b| b.to_vec()));
        }
        cache.populate(store, key, fetched.clone());
        Ok(fetched)
    }

    async fn s_set(&self, store: &str, key: &str, value: &[u8]) -> Result<()> {
        if let Some(wb) = &self.write_back {
            wb.lock().await.write(store, key, value.to_vec());
            return Ok(());
        }
        self.set_raw(store, key, value).await
    }

    async fn s_delete(&self, store: &str, key: &str) -> Result<()> {
        if let Some(wb) = &self.write_back {
            wb.lock().await.delete(store, key);
            return Ok(());
        }
        self.delete_raw(store, key).await
    }

    async fn s_set_many(&self, store: &str, entries: &[(String, Vec<u8>)]) -> Result<bool> {
        if let Some(wb) = &self.write_back {
            let mut cache = wb.lock().await;
            for (k, v) in entries {
                cache.write(store, k, v.clone());
            }
            return Ok(true); // absorbed; the actual batch crosses at flush
        }
        self.set_many_raw(store, entries).await
    }

    async fn s_delete_many(&self, store: &str, keys: &[String]) -> Result<bool> {
        if let Some(wb) = &self.write_back {
            let mut cache = wb.lock().await;
            for k in keys {
                cache.delete(store, k);
            }
            return Ok(true);
        }
        self.delete_many_raw(store, keys).await
    }

    async fn s_get_many(&self, store: &str, keys: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
        let Some(wb) = &self.write_back else {
            return self.get_many_raw(store, keys).await;
        };
        let mut out: Vec<(String, Vec<u8>)> = Vec::new();
        let mut misses: Vec<String> = Vec::new();
        {
            let cache = wb.lock().await;
            for k in keys {
                match cache.lookup(store, k) {
                    Some(Some(v)) => out.push((k.clone(), v.to_vec())),
                    Some(None) => {} // tombstone → omitted (absent)
                    None => misses.push(k.clone()),
                }
            }
        }
        if !misses.is_empty() {
            let fetched = self.get_many_raw(store, &misses).await?;
            let fetched_map: std::collections::HashMap<&str, &Vec<u8>> =
                fetched.iter().map(|(k, v)| (k.as_str(), v)).collect();
            let mut cache = wb.lock().await;
            for k in &misses {
                // A write/delete may have landed during the await — the cache
                // wins over the now-stale backend read (mirrors s_get's re-check).
                match cache.lookup(store, k) {
                    Some(Some(v)) => out.push((k.clone(), v.to_vec())),
                    Some(None) => {} // tombstoned during the await → absent
                    None => match fetched_map.get(k.as_str()) {
                        Some(v) => {
                            cache.populate(store, k, Some((*v).clone()));
                            out.push((k.clone(), (*v).clone()));
                        }
                        // Backend didn't have it either → negative-cache so a
                        // repeat lookup doesn't re-cross the boundary.
                        None => cache.populate(store, k, None),
                    },
                }
            }
        }
        Ok(out)
    }

    async fn s_list_keys(&self, store: &str, prefix: Option<&str>) -> Result<Vec<String>> {
        let backend_keys = self.list_keys_raw(store, prefix).await?;
        let Some(wb) = &self.write_back else {
            return Ok(backend_keys);
        };
        let cache = wb.lock().await;
        Ok(cache.merge_keys(store, backend_keys, prefix))
    }

    async fn s_delete_prefix(&self, store: &str, prefix: &str) -> Result<Option<u32>> {
        let Some(wb) = &self.write_back else {
            return self.delete_prefix_raw(store, prefix).await;
        };
        // Need the full matching key set (backend + un-flushed cache) so the
        // delete also tombstones cache-only keys. Prefer the enumerate path; if
        // the host can't list, delete from the backend now and tombstone the
        // cache's own matching keys. (js_backend.rs:298)
        if self.has_enumerate {
            let keys = self.s_list_keys(store, Some(prefix)).await?;
            let mut cache = wb.lock().await;
            for k in &keys {
                cache.delete(store, k);
            }
            Ok(Some(keys.len() as u32))
        } else {
            let backend_count = self.delete_prefix_raw(store, prefix).await?;
            let mut cache = wb.lock().await;
            let cache_count = cache.tombstone_prefix(store, prefix);
            Ok(Some(backend_count.unwrap_or(0).max(cache_count)))
        }
    }

    /// Flush all pending write-back entries to the host store (one batched
    /// crossing per namespace, per-key fallback when no batch handle). No-op
    /// when write-back is off or nothing is dirty. Called on disconnect/logout/
    /// shutdown so the host store ends up consistent. (js_backend.rs:325)
    ///
    /// Exposed via the public [`CoreBackend::flush_cache`] wrapper which returns
    /// `anyhow::Result` for the engine binding.
    async fn flush_cache_inner(&self) -> Result<()> {
        let Some(wb) = &self.write_back else {
            return Ok(());
        };
        let batch = {
            let cache = wb.lock().await;
            if cache.dirty_is_empty() {
                return Ok(());
            }
            cache.snapshot_for_flush()
        };
        // Writes first. A failure propagates via `?` BEFORE commit_flush, so the
        // dirty set + tombstones stay intact and the batch is retried on the next
        // flush (disconnect → logout → Drop give several chances).
        for (store, entries) in &batch.sets {
            if !self.set_many_raw(store, entries).await? {
                for (k, v) in entries {
                    self.set_raw(store, k, v).await?;
                }
            }
        }
        for (store, keys) in &batch.deletes {
            if !self.delete_many_raw(store, keys).await? {
                for k in keys {
                    self.delete_raw(store, k).await?;
                }
            }
        }
        // All writes landed → drop the flushed keys from the dirty set.
        wb.lock().await.commit_flush(&batch);
        Ok(())
    }

    /// Public flush entry point for the engine binding. No-op when write-back is
    /// disabled. Errors are surfaced as `anyhow` (keeping the `StoreError`
    /// context via `Display`).
    pub async fn flush_cache(&self) -> anyhow::Result<()> {
        self.flush_cache_inner()
            .await
            .map_err(|e| anyhow::anyhow!("write-back flush failed: {e}"))
    }

    // ── Host call helpers ─────────────────────────────────────────────────
    //
    // These replace the old `js_*_raw` functions. `HostStore` is already typed
    // (`Vec<u8>`/`String`) and returns `anyhow::Result`, so there is no JsValue
    // parsing / promise resolution / strict entry-array parsing to do here —
    // only error mapping into `StoreError` while preserving a context string.

    async fn get_raw(&self, store: &str, key: &str) -> Result<Option<Vec<u8>>> {
        self.host
            .get(store, key)
            .await
            .map_err(|e| host_err("get", e))
    }

    async fn set_raw(&self, store: &str, key: &str, value: &[u8]) -> Result<()> {
        self.host
            .set(store, key, value)
            .await
            .map_err(|e| host_err("set", e))
    }

    async fn delete_raw(&self, store: &str, key: &str) -> Result<()> {
        self.host
            .delete(store, key)
            .await
            .map_err(|e| host_err("delete", e))
    }

    /// Batch-write `[(key, value)]` into one store. Returns `Ok(true)` when the
    /// host handled the whole batch in one crossing; `Ok(false)` when no batch
    /// handle exists, so the caller falls back to per-key `s_set`/`set_raw`.
    /// (js_backend.rs:412)
    async fn set_many_raw(&self, store: &str, entries: &[(String, Vec<u8>)]) -> Result<bool> {
        if !self.has_batch {
            return Ok(false);
        }
        self.host
            .set_many(store, entries)
            .await
            .map_err(|e| host_err("setMany", e))
    }

    /// Batch-delete `keys` from one store. Returns `Ok(true)` when handled in one
    /// crossing; `Ok(false)` when no batch handle, so the caller falls back to
    /// per-key `delete_raw`. (js_backend.rs:436)
    async fn delete_many_raw(&self, store: &str, keys: &[String]) -> Result<bool> {
        if !self.has_batch {
            return Ok(false);
        }
        self.host
            .delete_many(store, keys)
            .await
            .map_err(|e| host_err("deleteMany", e))
    }

    /// Read many keys from one store in a single crossing via `get_many`. Falls
    /// back to a per-key `get_raw` loop when the host has no batch handle.
    /// Returns only FOUND entries (missing keys are omitted). (js_backend.rs:456)
    async fn get_many_raw(&self, store: &str, keys: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        if self.has_batch
            && let Some(found) = self
                .host
                .get_many(store, keys)
                .await
                .map_err(|e| host_err("getMany", e))?
        {
            return Ok(found);
        }
        // Fallback: per-key gets (raw — this IS the raw path; the cache-aware
        // s_get_many handles caching before delegating here).
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some(v) = self.get_raw(store, k).await? {
                out.push((k.clone(), v));
            }
        }
        Ok(out)
    }

    /// Enumerate live keys in `store` (optionally prefix-filtered) via the host's
    /// `list_keys`. Only valid when `has_enumerate`; callers gate on it.
    /// (js_backend.rs:490)
    async fn list_keys_raw(&self, store: &str, prefix: Option<&str>) -> Result<Vec<String>> {
        if !self.has_enumerate {
            return Err(host_err_msg("listKeys", "not available"));
        }
        // HostStore is already typed `Vec<String>`, so there is no per-element
        // strict string check to do (the wasm version validated JsValues); a
        // typed host cannot return a non-string key.
        self.host
            .list_keys(store, prefix)
            .await
            .map_err(|e| host_err("listKeys", e))
    }

    /// Delete every key in `store` starting with `prefix` via `delete_prefix`.
    /// Returns `Ok(Some(count))` when handled, `Ok(None)` when no handle exists.
    /// (js_backend.rs:524)
    async fn delete_prefix_raw(&self, store: &str, prefix: &str) -> Result<Option<u32>> {
        if !self.has_prefix_delete {
            return Ok(None);
        }
        self.host
            .delete_prefix(store, prefix)
            .await
            .map_err(|e| host_err("deletePrefix", e))
    }

    /// All keys currently in `store`. On the enumerate path this lists the store
    /// directly; otherwise it reads the hand-maintained JSON index under
    /// `STORE_META[meta_key]`. Single choke point shared by every `get_all_*` /
    /// `delete_expired_*` method across both host profiles. (js_backend.rs:542)
    async fn all_keys(&self, store: &str, meta_key: &str) -> Result<Vec<String>> {
        if self.has_enumerate {
            self.s_list_keys(store, None).await
        } else {
            Ok(self
                .get_json(STORE_META, meta_key)
                .await?
                .unwrap_or_default())
        }
    }

    /// Whether the core must maintain the JSON meta index for this backend.
    /// True only when the host cannot enumerate. (js_backend.rs:555)
    fn needs_self_index(&self) -> bool {
        !self.has_enumerate
    }

    /// Scan `keys` of a timestamp-prefixed store (8-byte BE seconds prefix) in
    /// bounded chunks, classifying each into (expired victims, live survivors).
    /// Values are pulled via `s_get_many` SCAN_CHUNK at a time. Keys already gone
    /// from the store fall out of both sets; values shorter than the 8-byte
    /// prefix are treated as corrupted victims. (js_backend.rs:566)
    async fn scan_expired(
        &self,
        store: &str,
        keys: &[String],
        cutoff_timestamp: i64,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let mut victims = Vec::new();
        let mut survivors = Vec::new();
        for chunk in keys.chunks(SCAN_CHUNK) {
            let found = self.s_get_many(store, chunk).await?;
            let map: std::collections::HashMap<&str, &Vec<u8>> =
                found.iter().map(|(k, v)| (k.as_str(), v)).collect();
            for key in chunk {
                match map.get(key.as_str()) {
                    Some(data) if data.len() >= 8 => {
                        let ts = i64::from_be_bytes(data[..8].try_into().unwrap_or([0; 8]));
                        if ts < cutoff_timestamp {
                            victims.push(key.clone());
                        } else {
                            survivors.push(key.clone());
                        }
                    }
                    Some(_) => victims.push(key.clone()),
                    None => {}
                }
            }
        }
        Ok((victims, survivors))
    }

    /// Re-read `victims` immediately before deletion and split them into
    /// `(still_expired, revived)`. A `revived` key gained a fresh timestamp (a
    /// concurrent `put` between classification and now) and MUST NOT be deleted
    /// nor dropped from the self-index. Narrows the scan→delete TOCTOU window to
    /// the gap between this re-read and the delete. (js_backend.rs:603)
    async fn confirm_expired(
        &self,
        store: &str,
        victims: Vec<String>,
        cutoff_timestamp: i64,
    ) -> Result<(Vec<String>, Vec<String>)> {
        if victims.is_empty() {
            return Ok((victims, Vec::new()));
        }
        let mut still = Vec::new();
        let mut revived = Vec::new();
        for chunk in victims.chunks(SCAN_CHUNK) {
            let found = self.s_get_many(store, chunk).await?;
            let map: std::collections::HashMap<&str, &Vec<u8>> =
                found.iter().map(|(k, v)| (k.as_str(), v)).collect();
            for key in chunk {
                match map.get(key.as_str()) {
                    Some(data) if data.len() >= 8 => {
                        let ts = i64::from_be_bytes(data[..8].try_into().unwrap_or([0; 8]));
                        if ts < cutoff_timestamp {
                            still.push(key.clone());
                        } else {
                            revived.push(key.clone());
                        }
                    }
                    // Corrupt → still delete. Missing → already gone (neither).
                    Some(_) => still.push(key.clone()),
                    None => {}
                }
            }
        }
        Ok((still, revived))
    }

    // ── Serialization helpers (js_backend.rs:639) ─────────────────────────

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        store: &str,
        key: &str,
    ) -> Result<Option<T>> {
        match self.s_get(store, key).await? {
            Some(bytes) => {
                let value: T = serde_json::from_slice(&bytes).map_err(|e| {
                    wacore::store::error::StoreError::Serialization(Box::new(JsonStoreError {
                        op: "deserialize",
                        store: store.to_string(),
                        key: key.to_string(),
                        source: e,
                    }))
                })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    async fn set_json<T: serde::Serialize>(&self, store: &str, key: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| {
            wacore::store::error::StoreError::Serialization(Box::new(JsonStoreError {
                op: "serialize",
                store: store.to_string(),
                key: key.to_string(),
                source: e,
            }))
        })?;
        self.s_set(store, key, &bytes).await
    }
}

// ---------------------------------------------------------------------------
// SignalStore (js_backend.rs:684)
// ---------------------------------------------------------------------------

#[async_trait]
impl SignalStore for CoreBackend {
    async fn put_identity(&self, address: &str, key: [u8; 32]) -> Result<()> {
        self.s_set(STORE_IDENTITY, address, &key).await
    }

    async fn load_identity(&self, address: &str) -> Result<Option<[u8; 32]>> {
        match self.s_get(STORE_IDENTITY, address).await? {
            Some(bytes) => Ok(Some(bytes.try_into().map_err(|v: Vec<u8>| {
                wacore::store::error::StoreError::Validation(format!(
                    "identity key for {address} has invalid length {}",
                    v.len()
                ))
            })?)),
            None => Ok(None),
        }
    }

    async fn delete_identity(&self, address: &str) -> Result<()> {
        self.s_delete(STORE_IDENTITY, address).await
    }

    async fn get_session(&self, address: &str) -> Result<Option<Bytes>> {
        Ok(self.s_get(STORE_SESSION, address).await?.map(Bytes::from))
    }

    async fn put_session(&self, address: &str, session: &[u8]) -> Result<()> {
        self.s_set(STORE_SESSION, address, session).await
    }

    async fn delete_session(&self, address: &str) -> Result<()> {
        self.s_delete(STORE_SESSION, address).await
    }

    async fn store_prekey(&self, id: u32, record: &[u8], _uploaded: bool) -> Result<()> {
        self.s_set(STORE_PREKEY, &id.to_string(), record).await
    }

    async fn load_prekey(&self, id: u32) -> Result<Option<Bytes>> {
        Ok(self
            .s_get(STORE_PREKEY, &id.to_string())
            .await?
            .map(Bytes::from))
    }

    async fn remove_prekey(&self, id: u32) -> Result<()> {
        self.s_delete(STORE_PREKEY, &id.to_string()).await
    }

    async fn get_max_prekey_id(&self) -> Result<u32> {
        match self.s_get(STORE_META, "max_prekey_id").await? {
            Some(bytes) => {
                let s = String::from_utf8(bytes).unwrap_or_default();
                Ok(s.parse::<u32>().unwrap_or(0))
            }
            None => Ok(0),
        }
    }

    async fn store_prekeys_batch(&self, keys: &[(u32, Bytes)], uploaded: bool) -> Result<()> {
        let mut max_id = self.get_max_prekey_id().await?;
        for (id, record) in keys {
            self.store_prekey(*id, record, uploaded).await?;
            if *id > max_id {
                max_id = *id;
            }
        }
        self.s_set(STORE_META, "max_prekey_id", max_id.to_string().as_bytes())
            .await?;
        Ok(())
    }

    async fn store_signed_prekey(&self, id: u32, record: &[u8]) -> Result<()> {
        self.s_set(STORE_SIGNED_PREKEY, &id.to_string(), record)
            .await?;
        if self.needs_self_index() {
            let mut ids = self.get_signed_prekey_ids().await?;
            if !ids.contains(&id) {
                ids.push(id);
                self.set_json(STORE_META, "signed_prekey_ids", &ids).await?;
            }
        }
        Ok(())
    }

    async fn load_signed_prekey(&self, id: u32) -> Result<Option<Vec<u8>>> {
        self.s_get(STORE_SIGNED_PREKEY, &id.to_string()).await
    }

    async fn load_all_signed_prekeys(&self) -> Result<Vec<(u32, Vec<u8>)>> {
        let ids = self.get_signed_prekey_ids().await?;
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(record) = self.load_signed_prekey(id).await? {
                result.push((id, record));
            }
        }
        Ok(result)
    }

    async fn remove_signed_prekey(&self, id: u32) -> Result<()> {
        self.s_delete(STORE_SIGNED_PREKEY, &id.to_string()).await?;
        if self.needs_self_index() {
            let mut ids = self.get_signed_prekey_ids().await?;
            ids.retain(|&i| i != id);
            self.set_json(STORE_META, "signed_prekey_ids", &ids).await?;
        }
        Ok(())
    }

    async fn put_sender_key(&self, address: &str, record: &[u8]) -> Result<()> {
        self.s_set(STORE_SENDER_KEY, address, record).await
    }

    async fn get_sender_key(&self, address: &str) -> Result<Option<Vec<u8>>> {
        self.s_get(STORE_SENDER_KEY, address).await
    }

    async fn delete_sender_key(&self, address: &str) -> Result<()> {
        self.s_delete(STORE_SENDER_KEY, address).await
    }
}

impl CoreBackend {
    /// (js_backend.rs:809)
    async fn get_signed_prekey_ids(&self) -> Result<Vec<u32>> {
        if self.has_enumerate {
            // Derive ids from the store's keys (id.to_string()) directly.
            let keys = self.s_list_keys(STORE_SIGNED_PREKEY, None).await?;
            Ok(keys.iter().filter_map(|k| k.parse::<u32>().ok()).collect())
        } else {
            Ok(self
                .get_json::<Vec<u32>>(STORE_META, "signed_prekey_ids")
                .await?
                .unwrap_or_default())
        }
    }

    /// Mutation-MAC name index (legacy/no-enumerate path only). The old wasm
    /// code maintained NO such index because it had no `clear_mutation_macs`.
    /// This rev's trait REQUIRES `clear_mutation_macs`, and on a host that can
    /// neither enumerate nor prefix-delete the ONLY way to find every
    /// `"{name}:{hex}"` mutation-mac key is a per-name index we maintain
    /// ourselves. It is additive (written only when `needs_self_index()`), keeps
    /// every existing value key byte-identical, and is invisible to enumerate /
    /// prefix-delete hosts (which derive the key set from the store directly).
    /// Stored under `STORE_META` as `"mutation_mac_index:{name}"` → `Vec<String>`
    /// of the hex `index_mac` suffixes.
    async fn mutation_mac_index_add(&self, name: &str, index_mac_hex: &str) -> Result<()> {
        let meta_key = format!("mutation_mac_index:{name}");
        let mut hexes: Vec<String> = self
            .get_json(STORE_META, &meta_key)
            .await?
            .unwrap_or_default();
        if !hexes.iter().any(|h| h == index_mac_hex) {
            hexes.push(index_mac_hex.to_string());
            self.set_json(STORE_META, &meta_key, &hexes).await?;
        }
        Ok(())
    }

    async fn mutation_mac_index_remove(&self, name: &str, index_mac_hex: &str) -> Result<()> {
        let meta_key = format!("mutation_mac_index:{name}");
        let mut hexes: Vec<String> = self
            .get_json(STORE_META, &meta_key)
            .await?
            .unwrap_or_default();
        let before = hexes.len();
        hexes.retain(|h| h != index_mac_hex);
        if hexes.len() != before {
            self.set_json(STORE_META, &meta_key, &hexes).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AppSyncStore (js_backend.rs:829)
// ---------------------------------------------------------------------------

#[async_trait]
impl AppSyncStore for CoreBackend {
    async fn get_sync_key(&self, key_id: &[u8]) -> Result<Option<AppStateSyncKey>> {
        let hex_id = to_hex(key_id);
        self.get_json(STORE_SYNC_KEY, &hex_id).await
    }

    async fn set_sync_key(&self, key_id: &[u8], key: AppStateSyncKey) -> Result<()> {
        let hex_id = to_hex(key_id);
        self.set_json(STORE_SYNC_KEY, &hex_id, &key).await?;
        self.s_set(STORE_META, "latest_sync_key_id", key_id).await
    }

    async fn get_version(&self, name: &str) -> Result<HashState> {
        Ok(self
            .get_json(STORE_SYNC_VERSION, name)
            .await?
            .unwrap_or_default())
    }

    async fn set_version(&self, name: &str, state: HashState) -> Result<()> {
        self.set_json(STORE_SYNC_VERSION, name, &state).await
    }

    async fn put_mutation_macs(
        &self,
        name: &str,
        _version: u64,
        mutations: &[AppStateMutationMAC],
    ) -> Result<()> {
        let self_index = self.needs_self_index();
        for m in mutations {
            let hex = to_hex(&m.index_mac);
            let key = format!("{}:{}", name, hex);
            self.s_set(STORE_MUTATION_MAC, &key, &m.value_mac).await?;
            // Additive per-name index, legacy path only — powers
            // clear_mutation_macs when the host can neither enumerate nor
            // prefix-delete. Value bytes above are unchanged.
            if self_index {
                self.mutation_mac_index_add(name, &hex).await?;
            }
        }
        Ok(())
    }

    async fn get_mutation_mac(&self, name: &str, index_mac: &[u8]) -> Result<Option<Vec<u8>>> {
        let key = format!("{}:{}", name, to_hex(index_mac));
        self.s_get(STORE_MUTATION_MAC, &key).await
    }

    async fn delete_mutation_macs(&self, name: &str, index_macs: &[Vec<u8>]) -> Result<()> {
        let self_index = self.needs_self_index();
        for im in index_macs {
            let hex = to_hex(im);
            let key = format!("{}:{}", name, hex);
            self.s_delete(STORE_MUTATION_MAC, &key).await?;
            if self_index {
                self.mutation_mac_index_remove(name, &hex).await?;
            }
        }
        Ok(())
    }

    /// Delete every mutation MAC for `name`. NEW required method (no default at
    /// this wacore rev). Capability-gated:
    ///   1. prefix-delete host → one `delete_prefix(STORE_MUTATION_MAC, "{name}:")`.
    ///   2. enumerate host → `list_keys` with the `"{name}:"` prefix, then
    ///      `delete_many` (per-key fallback when no batch).
    ///   3. legacy host → drain the per-name index built in `put_mutation_macs`,
    ///      delete each `"{name}:{hex}"` key, then clear the index.
    /// All three target exactly the keys whose composite key starts with
    /// `"{name}:"`, matching the put-side key format 1:1.
    async fn clear_mutation_macs(&self, name: &str) -> Result<()> {
        let prefix = format!("{name}:");

        // 1. Prefix-delete fast path.
        if self.has_prefix_delete
            && self
                .s_delete_prefix(STORE_MUTATION_MAC, &prefix)
                .await?
                .is_some()
        {
            return Ok(());
        }

        // 2. Enumerate path.
        if self.has_enumerate {
            let keys = self.s_list_keys(STORE_MUTATION_MAC, Some(&prefix)).await?;
            if !keys.is_empty() && !self.s_delete_many(STORE_MUTATION_MAC, &keys).await? {
                for k in &keys {
                    self.s_delete(STORE_MUTATION_MAC, k).await?;
                }
            }
            return Ok(());
        }

        // 3. Legacy self-index path: reconstruct the composite keys from the
        //    per-name hex index, delete them, then drop the index.
        let meta_key = format!("mutation_mac_index:{name}");
        let hexes: Vec<String> = self
            .get_json(STORE_META, &meta_key)
            .await?
            .unwrap_or_default();
        if !hexes.is_empty() {
            let keys: Vec<String> = hexes.iter().map(|h| format!("{name}:{h}")).collect();
            if !self.s_delete_many(STORE_MUTATION_MAC, &keys).await? {
                for k in &keys {
                    self.s_delete(STORE_MUTATION_MAC, k).await?;
                }
            }
        }
        self.s_delete(STORE_META, &meta_key).await?;
        Ok(())
    }

    async fn get_latest_sync_key_id(&self) -> Result<Option<Vec<u8>>> {
        self.s_get(STORE_META, "latest_sync_key_id").await
    }
}

// ---------------------------------------------------------------------------
// ProtocolStore (js_backend.rs:889)
// ---------------------------------------------------------------------------

#[async_trait]
impl ProtocolStore for CoreBackend {
    // --- Sender Key Device Tracking ---

    async fn get_sender_key_devices(&self, group_jid: &str) -> Result<Vec<(String, bool)>> {
        Ok(self
            .get_json(STORE_SENDER_KEY_DEVICES, group_jid)
            .await?
            .unwrap_or_default())
    }

    async fn set_sender_key_status(&self, group_jid: &str, entries: &[(&str, bool)]) -> Result<()> {
        let mut devices: Vec<(String, bool)> = self.get_sender_key_devices(group_jid).await?;
        for &(jid, status) in entries {
            if let Some(existing) = devices.iter_mut().find(|(j, _)| j == jid) {
                existing.1 = status;
            } else {
                devices.push((jid.to_string(), status));
            }
        }
        self.set_json(STORE_SENDER_KEY_DEVICES, group_jid, &devices)
            .await?;
        // Track group JID so clear_all can enumerate them — only needed when the
        // host can't list the STORE_SENDER_KEY_DEVICES namespace itself.
        if self.needs_self_index() {
            let mut groups: Vec<String> = self
                .get_json(STORE_META, "sender_key_groups")
                .await?
                .unwrap_or_default();
            if !groups.iter().any(|g| g == group_jid) {
                groups.push(group_jid.to_string());
                self.set_json(STORE_META, "sender_key_groups", &groups)
                    .await?;
            }
        }
        Ok(())
    }

    async fn clear_sender_key_devices(&self, group_jid: &str) -> Result<()> {
        self.s_delete(STORE_SENDER_KEY_DEVICES, group_jid).await?;
        if self.needs_self_index() {
            // Remove from tracking list
            let mut groups: Vec<String> = self
                .get_json(STORE_META, "sender_key_groups")
                .await?
                .unwrap_or_default();
            if let Some(pos) = groups.iter().position(|g| g == group_jid) {
                groups.swap_remove(pos);
                self.set_json(STORE_META, "sender_key_groups", &groups)
                    .await?;
            }
        }
        Ok(())
    }

    async fn delete_sender_key_device_rows(&self, device_jids: &[&str]) -> Result<()> {
        if device_jids.is_empty() {
            return Ok(());
        }
        let targets: std::collections::HashSet<&str> = device_jids.iter().copied().collect();
        let groups = self
            .all_keys(STORE_SENDER_KEY_DEVICES, "sender_key_groups")
            .await?;
        for group in &groups {
            let mut devices: Vec<(String, bool)> = self
                .get_json(STORE_SENDER_KEY_DEVICES, group)
                .await?
                .unwrap_or_default();
            let before = devices.len();
            devices.retain(|(jid, _)| !targets.contains(jid.as_str()));
            if devices.len() != before {
                self.set_json(STORE_SENDER_KEY_DEVICES, group, &devices)
                    .await?;
            }
        }
        Ok(())
    }

    async fn clear_all_sender_key_devices(&self) -> Result<()> {
        // The STORE_SENDER_KEY_DEVICES namespace holds only per-group rows, so a
        // whole-namespace prefix delete clears everything when available.
        if !(self.has_prefix_delete
            && self
                .s_delete_prefix(STORE_SENDER_KEY_DEVICES, "")
                .await?
                .is_some())
        {
            let groups = self
                .all_keys(STORE_SENDER_KEY_DEVICES, "sender_key_groups")
                .await?;
            if !groups.is_empty()
                && !self
                    .s_delete_many(STORE_SENDER_KEY_DEVICES, &groups)
                    .await?
            {
                for group in &groups {
                    self.s_delete(STORE_SENDER_KEY_DEVICES, group).await?;
                }
            }
        }
        if self.needs_self_index() {
            self.s_delete(STORE_META, "sender_key_groups").await?;
        }
        Ok(())
    }

    // --- LID-PN Mapping ---

    async fn get_lid_mapping(&self, lid: &str) -> Result<Option<LidPnMappingEntry>> {
        self.get_json(STORE_LID_MAPPING, &format!("lid:{lid}"))
            .await
    }

    async fn get_pn_mapping(&self, phone: &str) -> Result<Option<LidPnMappingEntry>> {
        match self
            .s_get(STORE_LID_MAPPING, &format!("pn:{phone}"))
            .await?
        {
            Some(lid_bytes) => {
                let lid = String::from_utf8(lid_bytes).unwrap_or_default();
                self.get_lid_mapping(&lid).await
            }
            None => Ok(None),
        }
    }

    async fn put_lid_mapping(&self, entry: &LidPnMappingEntry) -> Result<()> {
        // Remove stale reverse entry if phone number changed
        if let Some(old_entry) = self.get_lid_mapping(&entry.lid).await?
            && old_entry.phone_number != entry.phone_number
        {
            self.s_delete(STORE_LID_MAPPING, &format!("pn:{}", old_entry.phone_number))
                .await?;
        }
        // Forward mapping (lid -> entry)
        self.set_json(STORE_LID_MAPPING, &format!("lid:{}", entry.lid), entry)
            .await?;
        // Reverse mapping (pn -> lid)
        self.s_set(
            STORE_LID_MAPPING,
            &format!("pn:{}", entry.phone_number),
            entry.lid.as_bytes(),
        )
        .await?;
        // `lid_list` tracks bare lids for get_all; only needed when the host
        // can't enumerate the `lid:`-prefixed keys itself.
        if self.needs_self_index() {
            let mut lids: Vec<String> = self
                .get_json(STORE_META, "lid_list")
                .await?
                .unwrap_or_default();
            if !lids.contains(&entry.lid) {
                lids.push(entry.lid.clone());
                self.set_json(STORE_META, "lid_list", &lids).await?;
            }
        }
        Ok(())
    }

    async fn get_all_lid_mappings(&self) -> Result<Vec<LidPnMappingEntry>> {
        // Enumerate the `lid:`-prefixed keys directly, or fall back to the
        // bare-lid index. Either way, resolve each via get_lid_mapping.
        let lids: Vec<String> = if self.has_enumerate {
            self.s_list_keys(STORE_LID_MAPPING, Some("lid:"))
                .await?
                .into_iter()
                .filter_map(|k| k.strip_prefix("lid:").map(str::to_string))
                .collect()
        } else {
            self.get_json(STORE_META, "lid_list")
                .await?
                .unwrap_or_default()
        };
        let mut result = Vec::with_capacity(lids.len());
        for lid in lids {
            if let Some(entry) = self.get_lid_mapping(&lid).await? {
                result.push(entry);
            }
        }
        Ok(result)
    }

    // --- Base Key Collision Detection ---

    async fn save_base_key(&self, address: &str, message_id: &str, base_key: &[u8]) -> Result<()> {
        let key = format!("{address}:{message_id}");
        self.s_set(STORE_BASE_KEY, &key, base_key).await
    }

    async fn has_same_base_key(
        &self,
        address: &str,
        message_id: &str,
        current_base_key: &[u8],
    ) -> Result<bool> {
        let key = format!("{address}:{message_id}");
        match self.s_get(STORE_BASE_KEY, &key).await? {
            Some(stored) => Ok(stored == current_base_key),
            None => Ok(false),
        }
    }

    async fn delete_base_key(&self, address: &str, message_id: &str) -> Result<()> {
        let key = format!("{address}:{message_id}");
        self.s_delete(STORE_BASE_KEY, &key).await
    }

    // --- Device Registry ---

    async fn update_device_list(&self, record: DeviceListRecord) -> Result<()> {
        self.set_json(STORE_DEVICE_LIST, &record.user, &record)
            .await
    }

    async fn get_devices(&self, user: &str) -> Result<Option<DeviceListRecord>> {
        self.get_json(STORE_DEVICE_LIST, user).await
    }

    async fn delete_devices(&self, user: &str) -> Result<()> {
        self.s_delete(STORE_DEVICE_LIST, user).await
    }

    // --- TcToken Storage ---

    async fn get_tc_token(&self, jid: &str) -> Result<Option<TcTokenEntry>> {
        self.get_json(STORE_TC_TOKEN, jid).await
    }

    async fn put_tc_token(&self, jid: &str, entry: &TcTokenEntry) -> Result<()> {
        self.set_json(STORE_TC_TOKEN, jid, entry).await?;
        let mut jids: Vec<String> = self
            .get_json(STORE_META, "tc_token_jids")
            .await?
            .unwrap_or_default();
        if !jids.iter().any(|j| j == jid) {
            jids.push(jid.to_string());
            self.set_json(STORE_META, "tc_token_jids", &jids).await?;
        }
        Ok(())
    }

    async fn delete_tc_token(&self, jid: &str) -> Result<()> {
        self.s_delete(STORE_TC_TOKEN, jid).await?;
        let mut jids: Vec<String> = self
            .get_json(STORE_META, "tc_token_jids")
            .await?
            .unwrap_or_default();
        jids.retain(|j| j != jid);
        self.set_json(STORE_META, "tc_token_jids", &jids).await
    }

    async fn get_all_tc_token_jids(&self) -> Result<Vec<String>> {
        Ok(self
            .get_json(STORE_META, "tc_token_jids")
            .await?
            .unwrap_or_default())
    }

    async fn delete_expired_tc_tokens(&self, cutoff_timestamp: i64) -> Result<u32> {
        let jids = self.get_all_tc_token_jids().await?;
        let mut deleted = 0u32;
        let mut remaining_jids = Vec::new();
        for jid in jids {
            if let Some(entry) = self.get_json::<TcTokenEntry>(STORE_TC_TOKEN, &jid).await? {
                if entry.token_timestamp < cutoff_timestamp {
                    self.s_delete(STORE_TC_TOKEN, &jid).await?;
                    deleted += 1;
                } else {
                    remaining_jids.push(jid);
                }
            }
        }
        self.set_json(STORE_META, "tc_token_jids", &remaining_jids)
            .await?;
        Ok(deleted)
    }

    // --- Sent Message Store ---

    async fn store_sent_message(
        &self,
        chat_jid: &str,
        message_id: &str,
        payload: &[u8],
    ) -> Result<()> {
        let key = format!("{chat_jid}:{message_id}");
        let now = wacore::time::now_secs();
        let mut data = Vec::with_capacity(8 + payload.len());
        data.extend_from_slice(&now.to_be_bytes());
        data.extend_from_slice(payload);
        self.s_set(STORE_SENT_MESSAGE, &key, &data).await?;

        // Self-index path keeps an in-memory key list (no serialization on the
        // hot path; flushed only on expiry). Enumerate-capable hosts skip it and
        // derive the key set from the store at expiry time.
        if self.needs_self_index() {
            let mut guard = self.get_sent_keys().await?;
            if let Some(ref mut keys) = *guard {
                keys.push(key);
            }
        }
        Ok(())
    }

    async fn take_sent_message(&self, chat_jid: &str, message_id: &str) -> Result<Option<Vec<u8>>> {
        let key = format!("{chat_jid}:{message_id}");

        // Fetch and delete from store WITHOUT holding the mutex
        let data = match self.s_get(STORE_SENT_MESSAGE, &key).await? {
            Some(data) if data.len() > 8 => data,
            _ => return Ok(None),
        };
        self.s_delete(STORE_SENT_MESSAGE, &key).await?;

        // Brief lock to update in-memory index (self-index path only).
        if self.needs_self_index() {
            let mut guard = self.sent_message_keys.lock().await;
            if let Some(ref mut keys) = *guard {
                keys.retain(|k| k != &key);
            }
        }

        // Skip 8-byte timestamp prefix
        Ok(Some(data[8..].to_vec()))
    }

    async fn delete_expired_sent_messages(&self, cutoff_timestamp: i64) -> Result<u32> {
        // Enumerate-capable hosts: list the namespace and scan in bounded
        // chunks — no in-memory index to maintain.
        if self.has_enumerate {
            let keys = self.s_list_keys(STORE_SENT_MESSAGE, None).await?;
            let (victims, _survivors) = self
                .scan_expired(STORE_SENT_MESSAGE, &keys, cutoff_timestamp)
                .await?;
            // Re-validate right before delete so a concurrently-rewritten entry
            // (fresh timestamp) isn't deleted. No self-index on this path, so
            // we only need the still-expired set.
            let (victims, _revived) = self
                .confirm_expired(STORE_SENT_MESSAGE, victims, cutoff_timestamp)
                .await?;
            if !victims.is_empty() && !self.s_delete_many(STORE_SENT_MESSAGE, &victims).await? {
                for key in &victims {
                    self.s_delete(STORE_SENT_MESSAGE, key).await?;
                }
            }
            return Ok(victims.len() as u32);
        }

        let mut guard = self.get_sent_keys().await?;
        let keys = match guard.as_mut() {
            Some(k) => k,
            None => return Ok(0),
        };

        let mut deleted = 0u32;
        // Collect indices to remove in reverse order to avoid shifting
        let mut to_remove = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            match self.s_get(STORE_SENT_MESSAGE, key).await {
                Ok(Some(data)) if data.len() >= 8 => {
                    let ts = i64::from_be_bytes(data[..8].try_into().unwrap_or([0; 8]));
                    if ts < cutoff_timestamp {
                        self.s_delete(STORE_SENT_MESSAGE, key).await?;
                        to_remove.push(i);
                        deleted += 1;
                    }
                }
                Ok(None) => {
                    // Key in index but not on disk — stale entry, remove
                    to_remove.push(i);
                }
                Ok(Some(_)) => {
                    // Data too short — corrupted, remove
                    self.s_delete(STORE_SENT_MESSAGE, key).await?;
                    to_remove.push(i);
                }
                Err(e) => {
                    log::warn!("Failed to check sent message {key}: {e}");
                    // Keep the key — don't lose it on transient errors
                }
            }
        }

        // Remove in reverse order to preserve indices
        for i in to_remove.into_iter().rev() {
            keys.swap_remove(i);
        }

        self.flush_sent_keys(keys).await?;
        Ok(deleted)
    }
}

// ---------------------------------------------------------------------------
// MsgSecretStore (js_backend.rs:1290)
// ---------------------------------------------------------------------------

#[async_trait]
impl MsgSecretStore for CoreBackend {
    async fn put_msg_secret(
        &self,
        chat: &str,
        sender: &str,
        msg_id: &str,
        secret: &[u8],
    ) -> Result<()> {
        let key = format!("{chat}:{sender}:{msg_id}");
        // 8-byte BE timestamp prefix powers delete_expired_msg_secrets.
        let now = wacore::time::now_secs();
        let mut data = Vec::with_capacity(8 + secret.len());
        data.extend_from_slice(&now.to_be_bytes());
        data.extend_from_slice(secret);
        self.s_set(STORE_MSG_SECRET, &key, &data).await?;

        // Self-index only when the host can't enumerate; an enumerate-capable
        // host derives the key set from the store directly (see all_keys).
        if self.needs_self_index() {
            let mut keys: Vec<String> = self
                .get_json(STORE_META, "msg_secret_keys")
                .await?
                .unwrap_or_default();
            if !keys.iter().any(|k| k == &key) {
                keys.push(key);
                self.set_json(STORE_META, "msg_secret_keys", &keys).await?;
            }
        }
        Ok(())
    }

    /// Batched variant. Builds the `[(key, value)]` batch once and writes it in a
    /// single `set_many` crossing (history sync delivers tens of thousands of
    /// secrets at once). The self-index is only maintained for non-enumerable
    /// hosts, and even then loaded/rewritten ONCE (HashSet dedupe → O(n)).
    async fn put_msg_secrets(&self, entries: Vec<MsgSecretEntry>) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        // Single timestamp for the batch; 8-byte BE prefix powers
        // delete_expired_msg_secrets (same format as put_msg_secret).
        let now_bytes = wacore::time::now_secs().to_be_bytes();
        let mut batch: Vec<(String, Vec<u8>)> = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = format!("{}:{}:{}", entry.chat, entry.sender, entry.msg_id);
            let mut data = Vec::with_capacity(8 + entry.secret.len());
            data.extend_from_slice(&now_bytes);
            data.extend_from_slice(&entry.secret);
            batch.push((key, data));
        }
        let stored = batch.len();

        // Self-index (non-enumerable hosts only): extend the index BEFORE the
        // value writes. Ordering matters for crash safety: a dangling index
        // entry (key indexed, value write later failed) is self-correcting —
        // delete_expired's scan treats a missing value as "drop from index". An
        // ORPHANED value (written but never indexed) is NOT self-correcting and
        // would live forever. So index-first trades a harmless transient for an
        // unbounded leak. Load once, dedupe via HashSet, rewrite once (O(n)).
        if self.needs_self_index() {
            let mut keys: Vec<String> = self
                .get_json(STORE_META, "msg_secret_keys")
                .await?
                .unwrap_or_default();
            let original_len = keys.len();
            let mut seen: std::collections::HashSet<String> = keys.iter().cloned().collect();
            for (key, _) in &batch {
                if seen.insert(key.clone()) {
                    keys.push(key.clone());
                }
            }
            if keys.len() != original_len {
                self.set_json(STORE_META, "msg_secret_keys", &keys).await?;
            }
        }

        // One crossing when the host implements set_many; otherwise per-key.
        if !self.s_set_many(STORE_MSG_SECRET, &batch).await? {
            for (key, data) in &batch {
                self.s_set(STORE_MSG_SECRET, key, data).await?;
            }
        }
        Ok(stored)
    }

    async fn get_msg_secret(
        &self,
        chat: &str,
        sender: &str,
        msg_id: &str,
    ) -> Result<Option<Vec<u8>>> {
        let key = format!("{chat}:{sender}:{msg_id}");
        // Strip the 8-byte timestamp prefix written by put_msg_secret.
        Ok(self
            .s_get(STORE_MSG_SECRET, &key)
            .await?
            .filter(|d| d.len() >= 8)
            .map(|d| d[8..].to_vec()))
    }

    async fn delete_expired_msg_secrets(&self, cutoff_timestamp: i64) -> Result<u32> {
        // Key set from enumeration (enumerate hosts) or the JSON index (legacy
        // hosts) — same code path either way.
        let keys = self.all_keys(STORE_MSG_SECRET, "msg_secret_keys").await?;
        let (victims, mut survivors) = self
            .scan_expired(STORE_MSG_SECRET, &keys, cutoff_timestamp)
            .await?;
        // Re-read victims right before deleting: a concurrent put may have
        // rewritten one with a fresh timestamp. `revived` keys are NOT deleted
        // and stay in the index.
        let (victims, revived) = self
            .confirm_expired(STORE_MSG_SECRET, victims, cutoff_timestamp)
            .await?;
        survivors.extend(revived);

        if !victims.is_empty() && !self.s_delete_many(STORE_MSG_SECRET, &victims).await? {
            for key in &victims {
                self.s_delete(STORE_MSG_SECRET, key).await?;
            }
        }

        // Rewrite the index to the survivors only on the self-index path, and
        // only if anything was actually removed (deleted or vanished).
        if self.needs_self_index() && survivors.len() != keys.len() {
            self.set_json(STORE_META, "msg_secret_keys", &survivors)
                .await?;
        }
        Ok(victims.len() as u32)
    }
}

// ---------------------------------------------------------------------------
// DeviceStore (js_backend.rs:1432)
// ---------------------------------------------------------------------------

#[async_trait]
impl DeviceStore for CoreBackend {
    async fn save(&self, device: &Device) -> Result<()> {
        self.set_json(STORE_DEVICE, "device", device).await?;

        // `account` (AdvSignedDeviceIdentity) is #[serde(skip)] in Device, so we
        // persist it separately as raw protobuf bytes — same approach as SQLite
        // storage which uses a dedicated column. `account_serde::to_bytes` is
        // literally `account.encode_to_vec()` (identical to the old wasm path),
        // so we avoid a direct `prost` dependency.
        if let Some(ref account) = device.account {
            let bytes = wacore::store::device::account_serde::to_bytes(account);
            self.s_set(STORE_DEVICE, "account", &bytes).await?;
        }

        Ok(())
    }

    async fn load(&self) -> Result<Option<Device>> {
        let mut device: Option<Device> = self.get_json(STORE_DEVICE, "device").await?;

        // Restore the #[serde(skip)] `account` field from its separate key.
        // `account_serde::from_bytes` is `AdvSignedDeviceIdentity::decode(..)`.
        if let Some(ref mut dev) = device
            && let Some(bytes) = self.s_get(STORE_DEVICE, "account").await?
        {
            match wacore::store::device::account_serde::from_bytes(&bytes) {
                Ok(account) => dev.account = Some(Arc::new(account)),
                Err(e) => log::warn!("Failed to decode stored account identity: {e}"),
            }
        }

        Ok(device)
    }

    async fn exists(&self) -> Result<bool> {
        Ok(self.s_get(STORE_DEVICE, "device").await?.is_some())
    }

    async fn create(&self) -> Result<i32> {
        let id = self.next_device_id.fetch_add(1, Ordering::Relaxed);
        // Materialize a default Device if none exists (same behavior as InMemoryBackend)
        if !self.exists().await? {
            self.save(&Device::new()).await?;
        }
        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// CoreCacheStore — port of js_cache_store.rs over Arc<dyn HostCacheStore>
// ---------------------------------------------------------------------------

/// Wraps an `Arc<dyn HostCacheStore>` to implement wacore's `CacheStore` trait.
/// Lets the engine binding provide a custom TTL-aware cache backend (Redis,
/// Memcached, etc.) while the Rust client uses it transparently. Port of the old
/// wasm `JsCacheStoreAdapter`.
pub struct CoreCacheStore {
    host: Arc<dyn HostCacheStore>,
}

impl CoreCacheStore {
    pub fn new(host: Arc<dyn HostCacheStore>) -> Self {
        Self { host }
    }
}

/// Build a `CoreCacheStore` as the wacore `CacheStore` trait object.
pub fn new_core_cache_store(host: Arc<dyn HostCacheStore>) -> Arc<dyn CacheStore> {
    Arc::new(CoreCacheStore::new(host))
}

#[async_trait]
impl CacheStore for CoreCacheStore {
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.host.get(namespace, key).await
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
        ttl: Option<std::time::Duration>,
    ) -> anyhow::Result<()> {
        // Match the old adapter: TTL crosses as seconds (`d.as_secs() as f64`),
        // `None` stays absent. (js_cache_store.rs:102)
        let ttl_secs = ttl.map(|d| d.as_secs() as f64);
        self.host.set(namespace, key, value, ttl_secs).await
    }

    async fn delete(&self, namespace: &str, key: &str) -> anyhow::Result<()> {
        self.host.delete(namespace, key).await
    }

    async fn clear(&self, namespace: &str) -> anyhow::Result<()> {
        self.host.clear(namespace).await
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Wraps a host-store callback failure with a context string, mapping into
/// `StoreError::Database`. Replaces the old `JsCallbackError` / `js_err_to_store_err`
/// (js_backend.rs:1519,1539). The `anyhow::Error` carries the host's original
/// message; the context names which operation failed.
#[derive(Debug, thiserror::Error)]
#[error("host store {context}: {message}")]
pub(crate) struct HostCallbackError {
    pub context: &'static str,
    pub message: String,
}

/// Wraps a `serde_json` (de)serialization failure with the `<store>/<key>`
/// context. Source chain preserves the original `serde_json::Error` for
/// downcast. (js_backend.rs:1529)
#[derive(Debug, thiserror::Error)]
#[error("{op} {store}/{key}")]
pub(crate) struct JsonStoreError {
    pub op: &'static str,
    pub store: String,
    pub key: String,
    #[source]
    pub source: serde_json::Error,
}

fn host_err(context: &'static str, e: anyhow::Error) -> wacore::store::error::StoreError {
    wacore::store::error::StoreError::Database(Box::new(HostCallbackError {
        context,
        // `{e:#}` includes the anyhow source chain, preserving the host's
        // original error detail.
        message: format!("{e:#}"),
    }))
}

fn host_err_msg(context: &'static str, message: &str) -> wacore::store::error::StoreError {
    wacore::store::error::StoreError::Database(Box::new(HostCallbackError {
        context,
        message: message.to_string(),
    }))
}

/// Simple hex encoding for byte slices (avoids adding a `hex` crate dependency).
/// (js_backend.rs:1545)
fn to_hex(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod send_sync_asserts {
    //! Compile-time proof that the engine-agnostic backend types are genuinely
    //! `Send + Sync + 'static` (no faking, no `?Send`).
    use super::*;

    fn assert_send_sync<T: Send + Sync + 'static>() {}

    #[test]
    fn backend_is_send_sync() {
        assert_send_sync::<CoreBackend>();
        assert_send_sync::<CoreCacheStore>();
        assert_send_sync::<Arc<dyn Backend>>();
        assert_send_sync::<Arc<dyn CacheStore>>();
    }
}
