//! JS storage backend adapter.
//!
//! Implements the full `Backend` trait (SignalStore + AppSyncStore + ProtocolStore + DeviceStore)
//! by delegating all storage operations to three JavaScript callback functions:
//!
//! - `get(store: string, key: string) -> Promise<Uint8Array | null>`
//! - `set(store: string, key: string, value: Uint8Array) -> Promise<void>`
//! - `delete(store: string, key: string) -> Promise<void>`
//!
//! Complex types (Device, AppStateSyncKey, HashState, etc.) are serialized as JSON bytes.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use async_trait::async_trait;
use bytes::Bytes;
use js_sys::{Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use whatsapp_rust::buffa::Message;
use whatsapp_rust::wacore;
use whatsapp_rust::wacore::appstate::hash::HashState;
use whatsapp_rust::wacore::appstate::processor::AppStateMutationMAC;
use whatsapp_rust::wacore::store::Device;
use whatsapp_rust::wacore::store::InMemoryBackend;
use whatsapp_rust::wacore::store::error::Result;
use whatsapp_rust::wacore::store::traits::*;
use whatsapp_rust::waproto;

// ---------------------------------------------------------------------------
// Store name constants
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
const META_SENT_MESSAGE_KEYS: &str = "sent_message_keys";
const META_MAX_PREKEY_ID: &str = "max_prekey_id";
const META_SIGNED_PREKEY_IDS: &str = "signed_prekey_ids";
const META_LATEST_SYNC_KEY_ID: &str = "latest_sync_key_id";
const META_SENDER_KEY_GROUPS: &str = "sender_key_groups";
const META_LID_LIST: &str = "lid_list";
const META_TC_TOKEN_JIDS: &str = "tc_token_jids";
const META_MSG_SECRET_KEYS: &str = "msg_secret_keys";
/// Self-index of mutation-MAC keys (`"<name>:<hex>"`) for hosts without enumeration.
const MUTATION_MAC_INDEX: &str = "mutation_mac_keys";
const DEVICE_RECORD: &str = "device";
const DEVICE_ACCOUNT: &str = "account";
const ALL_KEYS_PREFIX: &str = "";
const STORE_KEY_SEPARATOR: &str = ":";
const TIMESTAMP_PREFIX_LEN: usize = std::mem::size_of::<i64>();

/// Join store-key components with an exact-capacity allocation. Besides
/// avoiding formatting machinery on hot paths, exact capacity avoids carrying
/// spare allocation through batch and host-boundary calls.
#[inline]
fn compound_store_key<const N: usize>(parts: [&str; N]) -> String {
    let component_bytes = parts.iter().map(|part| part.len()).sum::<usize>();
    let separator_bytes = STORE_KEY_SEPARATOR
        .len()
        .saturating_mul(parts.len().saturating_sub(1));
    let mut key = String::with_capacity(component_bytes + separator_bytes);
    for (index, part) in parts.into_iter().enumerate() {
        if index != 0 {
            key.push_str(STORE_KEY_SEPARATOR);
        }
        key.push_str(part);
    }
    key
}

#[cfg(test)]
mod compound_store_key_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn joins_components_without_retaining_spare_capacity() {
        let key = compound_store_key(["chat", "sender", "message"]);
        assert_eq!(key, "chat:sender:message");
        assert_eq!(key.capacity(), key.len());
    }
}

#[inline]
fn signal_address_matches_user(address: &str, user: &str) -> bool {
    address
        .strip_prefix(user)
        .and_then(|rest| rest.as_bytes().first())
        .is_some_and(|separator| matches!(separator, b'@' | b':'))
}

// ---------------------------------------------------------------------------
// Public API: backend factory
// ---------------------------------------------------------------------------

/// Get a new InMemoryBackend instance (fallback when no JS store is provided).
pub(crate) fn new_in_memory_backend() -> Arc<dyn Backend> {
    Arc::new(InMemoryBackend::default())
}

/// Raw JS callback handles + capability flags pulled from the host store object.
/// Grouped into one struct because the optional surface grew past clarity as a
/// positional argument list.
pub(crate) struct JsBackendHandles {
    pub get_fn: js_sys::Function,
    pub set_fn: js_sys::Function,
    pub delete_fn: js_sys::Function,
    pub set_many_fn: Option<js_sys::Function>,
    pub delete_many_fn: Option<js_sys::Function>,
    pub get_many_fn: Option<js_sys::Function>,
    pub list_keys_fn: Option<js_sys::Function>,
    pub delete_prefix_fn: Option<js_sys::Function>,
    pub cap_enumerate: bool,
    pub cap_prefix_delete: bool,
}

/// Create a JsBackend from JS callback handles. The batch/enumeration handles
/// are optional — when absent the backend falls back to per-key `set`/`delete`
/// and its self-maintained JSON meta-indexes.
///
pub(crate) fn new_js_backend(handles: JsBackendHandles) -> Arc<JsBackend> {
    Arc::new(JsBackend::new(handles))
}

// ---------------------------------------------------------------------------
// JsBackend struct
// ---------------------------------------------------------------------------

/// Storage backend that delegates all persistence to JavaScript callbacks.
pub struct JsBackend {
    get_fn: js_sys::Function,
    set_fn: js_sys::Function,
    delete_fn: js_sys::Function,
    /// Optional batch/enumeration handles — present only when the JS host
    /// implements them. When absent, ops degrade to per-key handles and the
    /// self-maintained JSON meta-indexes.
    set_many_fn: Option<js_sys::Function>,
    delete_many_fn: Option<js_sys::Function>,
    get_many_fn: Option<js_sys::Function>,
    list_keys_fn: Option<js_sys::Function>,
    delete_prefix_fn: Option<js_sys::Function>,
    /// True when the host can enumerate a namespace (capabilities.enumerate +
    /// listKeys present). When true the core DROPS its hand-maintained key
    /// indexes and derives the key set from the store directly.
    has_enumerate: bool,
    /// True when the host implements deletePrefix (capabilities.prefixDelete).
    has_prefix_delete: bool,
    next_device_id: AtomicI32,
    /// In-memory cache of sent message keys — avoids O(n²) JSON re-serialization
    /// on every store_sent_message call. Loaded lazily on first access. Only
    /// used on the self-index path (`!has_enumerate`).
    sent_message_keys: async_lock::Mutex<Option<Vec<String>>>,
}

crate::wasm_send_sync!(JsBackend);

/// Chunk size for value-scanning enumeration (delete-expired sweeps). Bounds
/// how many values are materialized across the FFI boundary at once so a
/// 20k-entry namespace doesn't spike JS-side memory in a single call.
const SCAN_CHUNK: usize = 512;
impl JsBackend {
    fn new(h: JsBackendHandles) -> Self {
        // A capability is only honored when its required method is actually
        // present, so a misdeclared host degrades safely instead of calling a
        // missing function.
        let has_enumerate = h.cap_enumerate && h.list_keys_fn.is_some();
        let has_prefix_delete = h.cap_prefix_delete && h.delete_prefix_fn.is_some();
        Self {
            get_fn: h.get_fn,
            set_fn: h.set_fn,
            delete_fn: h.delete_fn,
            set_many_fn: h.set_many_fn,
            delete_many_fn: h.delete_many_fn,
            get_many_fn: h.get_many_fn,
            list_keys_fn: h.list_keys_fn,
            delete_prefix_fn: h.delete_prefix_fn,
            has_enumerate,
            has_prefix_delete,
            next_device_id: AtomicI32::new(1),
            sent_message_keys: async_lock::Mutex::new(None),
        }
    }

    /// Get or lazily load the sent message keys list.
    async fn get_sent_keys(&self) -> Result<async_lock::MutexGuard<'_, Option<Vec<String>>>> {
        let mut guard = self.sent_message_keys.lock().await;
        if guard.is_none() {
            let keys: Vec<String> = self
                .js_get_json(STORE_META, META_SENT_MESSAGE_KEYS)
                .await?
                .unwrap_or_default();
            *guard = Some(keys);
        }
        Ok(guard)
    }

    /// Persist the in-memory key list to JS store.
    /// Only called during cleanup/expiration — never on the send hot path.
    async fn flush_sent_keys(&self, keys: &Vec<String>) -> Result<()> {
        self.js_set_json(STORE_META, META_SENT_MESSAGE_KEYS, keys)
            .await
    }

    // ── Backend entry points ───────────────────────────────────────────────

    async fn js_get(&self, store: &str, key: &str) -> Result<Option<Vec<u8>>> {
        self.js_get_raw(store, key).await
    }

    async fn js_set(&self, store: &str, key: &str, value: &[u8]) -> Result<()> {
        self.js_set_raw(store, key, value).await
    }

    async fn js_delete(&self, store: &str, key: &str) -> Result<()> {
        self.js_delete_raw(store, key).await
    }

    /// Persist an owned byte batch, degrading to the per-key write-through
    /// primitive only when the host has no batch capability.
    async fn js_put_many_owned(&self, store: &str, entries: Vec<(String, Vec<u8>)>) -> Result<()> {
        if self.js_set_many_raw(store, &entries).await? {
            return Ok(());
        }
        for (key, value) in entries {
            self.js_set_raw(store, &key, &value).await?;
        }
        Ok(())
    }

    async fn js_delete_many(&self, store: &str, keys: &[String]) -> Result<bool> {
        self.js_delete_many_raw(store, keys).await
    }

    async fn js_get_many(&self, store: &str, keys: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
        self.js_get_many_raw(store, keys).await
    }

    async fn js_list_keys(&self, store: &str, prefix: Option<&str>) -> Result<Vec<String>> {
        self.js_list_keys_raw(store, prefix).await
    }

    async fn js_delete_prefix(&self, store: &str, prefix: &str) -> Result<Option<u32>> {
        self.js_delete_prefix_raw(store, prefix).await
    }

    // ── JS call helpers ──────────────────────────────────────────────────

    async fn js_get_raw(&self, store: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let result = self
            .get_fn
            .call2(&JsValue::NULL, &store.into(), &key.into())
            .map_err(|e| js_err_to_store_err("get", e))?;

        let resolved = resolve_promise(result)
            .await
            .map_err(|e| js_err_to_store_err("get", e))?;

        if resolved.is_null() || resolved.is_undefined() {
            return Ok(None);
        }

        if let Some(arr) = resolved.dyn_ref::<Uint8Array>() {
            Ok(Some(arr.to_vec()))
        } else {
            Ok(None)
        }
    }

    async fn js_set_raw(&self, store: &str, key: &str, value: &[u8]) -> Result<()> {
        let uint8 = Uint8Array::from(value);
        let result = self
            .set_fn
            .call3(&JsValue::NULL, &store.into(), &key.into(), &uint8.into())
            .map_err(|e| js_err_to_store_err("set", e))?;

        resolve_promise(result)
            .await
            .map_err(|e| js_err_to_store_err("set", e))?;

        Ok(())
    }

    async fn js_delete_raw(&self, store: &str, key: &str) -> Result<()> {
        let result = self
            .delete_fn
            .call2(&JsValue::NULL, &store.into(), &key.into())
            .map_err(|e| js_err_to_store_err("delete", e))?;

        resolve_promise(result)
            .await
            .map_err(|e| js_err_to_store_err("delete", e))?;

        Ok(())
    }

    /// Batch-write `[(key, value)]` into one store via the host's `setMany`
    /// callback. Returns `Ok(true)` when the host provided `setMany` (the whole
    /// batch crossed the FFI boundary once); `Ok(false)` when no batch handle
    /// exists, so the caller must fall back to per-key `js_set`.
    #[cfg_attr(
        feature = "memory-profiling",
        tracing::instrument(name = "bridge.store.set_many_ffi", level = "trace", skip_all)
    )]
    async fn js_set_many_raw(&self, store: &str, entries: &[(String, Vec<u8>)]) -> Result<bool> {
        let Some(f) = self.set_many_fn.as_ref() else {
            return Ok(false);
        };
        let arr = js_sys::Array::new();
        for (k, v) in entries {
            let tuple = js_sys::Array::new();
            tuple.push(&JsValue::from_str(k));
            let value = Uint8Array::from(v.as_slice());
            tuple.push(&value.into());
            arr.push(&tuple);
        }
        let result = f
            .call2(&JsValue::NULL, &store.into(), &arr.into())
            .map_err(|e| js_err_to_store_err("setMany", e))?;
        resolve_promise(result)
            .await
            .map_err(|e| js_err_to_store_err("setMany", e))?;
        Ok(true)
    }

    /// Batch-delete `keys` from one store via the host's `deleteMany` callback.
    /// Returns `Ok(true)` when handled in one crossing; `Ok(false)` when no
    /// batch handle exists, so the caller must fall back to per-key `js_delete`.
    async fn js_delete_many_raw(&self, store: &str, keys: &[String]) -> Result<bool> {
        let Some(f) = self.delete_many_fn.as_ref() else {
            return Ok(false);
        };
        let arr = js_sys::Array::new();
        for k in keys {
            arr.push(&JsValue::from_str(k));
        }
        let result = f
            .call2(&JsValue::NULL, &store.into(), &arr.into())
            .map_err(|e| js_err_to_store_err("deleteMany", e))?;
        resolve_promise(result)
            .await
            .map_err(|e| js_err_to_store_err("deleteMany", e))?;
        Ok(true)
    }

    /// Read many keys from one store in a single FFI crossing via `getMany`.
    /// Falls back to a per-key `js_get` loop when the host has no batch handle.
    /// Returns only FOUND entries (missing keys are omitted).
    async fn js_get_many_raw(
        &self,
        store: &str,
        keys: &[String],
    ) -> Result<Vec<(String, Vec<u8>)>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(f) = self.get_many_fn.as_ref() {
            let arr = js_sys::Array::new();
            for k in keys {
                arr.push(&JsValue::from_str(k));
            }
            let result = f
                .call2(&JsValue::NULL, &store.into(), &arr.into())
                .map_err(|e| js_err_to_store_err("getMany", e))?;
            let resolved = resolve_promise(result)
                .await
                .map_err(|e| js_err_to_store_err("getMany", e))?;
            return parse_entry_array(resolved, "getMany");
        }
        // Fallback: per-key gets (raw — this IS the raw path; the cache-aware
        // js_get_many handles caching before delegating here).
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            if let Some(v) = self.js_get_raw(store, k).await? {
                out.push((k.clone(), v));
            }
        }
        Ok(out)
    }

    /// Enumerate live keys in `store` (optionally prefix-filtered) via the
    /// host's `listKeys`. Only valid when `has_enumerate`; callers gate on it.
    async fn js_list_keys_raw(&self, store: &str, prefix: Option<&str>) -> Result<Vec<String>> {
        let f = self
            .list_keys_fn
            .as_ref()
            .ok_or_else(|| js_err_to_store_err("listKeys", JsValue::from_str("not available")))?;
        let prefix_arg = match prefix {
            Some(p) => JsValue::from_str(p),
            None => JsValue::UNDEFINED,
        };
        let result = f
            .call2(&JsValue::NULL, &store.into(), &prefix_arg)
            .map_err(|e| js_err_to_store_err("listKeys", e))?;
        let resolved = resolve_promise(result)
            .await
            .map_err(|e| js_err_to_store_err("listKeys", e))?;
        let arr: js_sys::Array = resolved
            .dyn_into()
            .map_err(|_| js_err_to_store_err("listKeys", JsValue::from_str("expected array")))?;
        // Strict parse: a non-string element means the host returned a
        // malformed key set. Silently dropping it would make the core believe a
        // live key is absent and prune it from the self-index, so FAIL instead.
        let mut out = Vec::with_capacity(arr.length() as usize);
        for v in arr.iter() {
            let key = v.as_string().ok_or_else(|| {
                js_err_to_store_err("listKeys", JsValue::from_str("non-string key in result"))
            })?;
            out.push(key);
        }
        Ok(out)
    }

    /// Delete every key in `store` starting with `prefix` via `deletePrefix`.
    /// Returns `Ok(Some(count))` when handled, `Ok(None)` when no handle exists
    /// (caller must fall back to enumerate-then-deleteMany).
    async fn js_delete_prefix_raw(&self, store: &str, prefix: &str) -> Result<Option<u32>> {
        let Some(f) = self.delete_prefix_fn.as_ref() else {
            return Ok(None);
        };
        let result = f
            .call2(&JsValue::NULL, &store.into(), &prefix.into())
            .map_err(|e| js_err_to_store_err("deletePrefix", e))?;
        let resolved = resolve_promise(result)
            .await
            .map_err(|e| js_err_to_store_err("deletePrefix", e))?;
        Ok(Some(resolved.as_f64().unwrap_or(0.0) as u32))
    }

    /// All keys currently in `store`. On the enumerate path this lists the
    /// store directly; otherwise it reads the hand-maintained JSON index under
    /// `STORE_META[meta_key]`. This is the single choke point that lets every
    /// `get_all_*` / `delete_expired_*` method share one code path across both
    /// host profiles.
    async fn all_keys(&self, store: &str, meta_key: &str) -> Result<Vec<String>> {
        if self.has_enumerate {
            self.js_list_keys(store, None).await
        } else {
            Ok(self
                .js_get_json(STORE_META, meta_key)
                .await?
                .unwrap_or_default())
        }
    }

    /// Whether the core must maintain the JSON meta index for this backend.
    /// True only when the host cannot enumerate.
    fn needs_self_index(&self) -> bool {
        !self.has_enumerate
    }

    /// Scan `keys` of a timestamp-prefixed store (i64 BE seconds prefix) in
    /// bounded chunks, classifying each into (expired victims, live survivors).
    /// Values are pulled via `js_get_many` SCAN_CHUNK at a time, so a large
    /// namespace never materializes every value at once. Keys already gone from
    /// the store fall out of both sets (so a survivors rewrite prunes them from
    /// the self-index); values shorter than the timestamp prefix are treated as
    /// corrupted victims.
    async fn scan_expired(
        &self,
        store: &str,
        keys: &[String],
        cutoff_timestamp: i64,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let mut victims = Vec::new();
        let mut survivors = Vec::new();
        for chunk in keys.chunks(SCAN_CHUNK) {
            let found = self.js_get_many(store, chunk).await?;
            let map: std::collections::HashMap<&str, &Vec<u8>> =
                found.iter().map(|(k, v)| (k.as_str(), v)).collect();
            for key in chunk {
                match map.get(key.as_str()) {
                    Some(data) if data.len() >= TIMESTAMP_PREFIX_LEN => {
                        let ts = i64::from_be_bytes(
                            data[..TIMESTAMP_PREFIX_LEN]
                                .try_into()
                                .unwrap_or([0; TIMESTAMP_PREFIX_LEN]),
                        );
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
    /// `(still_expired, revived)`. A `revived` key gained a fresh timestamp
    /// (a concurrent `put` between classification and now) and MUST NOT be
    /// deleted — nor dropped from the self-index. This narrows the
    /// scan→delete TOCTOU window to the gap between this re-read and the
    /// delete (the smallest achievable without a store-level compare-and-delete
    /// primitive). Bounded by `victims.len()` (small in steady state).
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
            let found = self.js_get_many(store, chunk).await?;
            let map: std::collections::HashMap<&str, &Vec<u8>> =
                found.iter().map(|(k, v)| (k.as_str(), v)).collect();
            for key in chunk {
                match map.get(key.as_str()) {
                    Some(data) if data.len() >= TIMESTAMP_PREFIX_LEN => {
                        let ts = i64::from_be_bytes(
                            data[..TIMESTAMP_PREFIX_LEN]
                                .try_into()
                                .unwrap_or([0; TIMESTAMP_PREFIX_LEN]),
                        );
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

    // ── Serialization helpers ────────────────────────────────────────────

    async fn js_get_json<T: serde::de::DeserializeOwned>(
        &self,
        store: &str,
        key: &str,
    ) -> Result<Option<T>> {
        match self.js_get(store, key).await? {
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

    async fn js_set_json<T: serde::Serialize>(
        &self,
        store: &str,
        key: &str,
        value: &T,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| {
            wacore::store::error::StoreError::Serialization(Box::new(JsonStoreError {
                op: "serialize",
                store: store.to_string(),
                key: key.to_string(),
                source: e,
            }))
        })?;
        self.js_set(store, key, &bytes).await
    }
}

// ---------------------------------------------------------------------------
// SignalStore
// ---------------------------------------------------------------------------

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SignalStore for JsBackend {
    async fn put_identity(&self, address: &str, key: [u8; 32]) -> Result<()> {
        self.js_set(STORE_IDENTITY, address, &key).await
    }

    async fn put_identities_batch(&self, identities: &[(Arc<str>, [u8; 32])]) -> Result<()> {
        let entries = identities
            .iter()
            .map(|(address, key)| (address.to_string(), key.to_vec()))
            .collect::<Vec<_>>();
        self.js_put_many_owned(STORE_IDENTITY, entries).await
    }

    async fn load_identity(&self, address: &str) -> Result<Option<[u8; 32]>> {
        match self.js_get(STORE_IDENTITY, address).await? {
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
        self.js_delete(STORE_IDENTITY, address).await
    }

    async fn get_session(&self, address: &str) -> Result<Option<Bytes>> {
        Ok(self.js_get(STORE_SESSION, address).await?.map(Bytes::from))
    }

    async fn put_session(&self, address: &str, session: &[u8]) -> Result<()> {
        self.js_set(STORE_SESSION, address, session).await
    }

    async fn put_sessions_batch(&self, sessions: &[(Arc<str>, Bytes)]) -> Result<()> {
        let entries = sessions
            .iter()
            .map(|(address, session)| (address.to_string(), session.to_vec()))
            .collect::<Vec<_>>();
        self.js_put_many_owned(STORE_SESSION, entries).await
    }

    async fn delete_session(&self, address: &str) -> Result<()> {
        self.js_delete(STORE_SESSION, address).await
    }

    async fn has_signal_state_for_user(&self, user: &str) -> Result<bool> {
        // Non-enumerating stores cannot prove absence, so preserve the trait's
        // conservative answer. Enumerating stores can avoid the core's full
        // PN->LID device scan without baking any address range into the bridge.
        if !self.has_enumerate {
            return Ok(true);
        }

        for store in [STORE_SESSION, STORE_IDENTITY] {
            if self
                .js_list_keys(store, Some(user))
                .await?
                .iter()
                .any(|address| signal_address_matches_user(address, user))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn store_prekey(&self, id: u32, record: &[u8], _uploaded: bool) -> Result<()> {
        self.js_set(STORE_PREKEY, &id.to_string(), record).await
    }

    async fn load_prekey(&self, id: u32) -> Result<Option<Bytes>> {
        Ok(self
            .js_get(STORE_PREKEY, &id.to_string())
            .await?
            .map(Bytes::from))
    }

    async fn load_prekeys_batch(&self, ids: &[u32]) -> Result<Vec<(u32, Bytes)>> {
        let keys = ids.iter().map(u32::to_string).collect::<Vec<_>>();
        self.js_get_many(STORE_PREKEY, &keys)
            .await?
            .into_iter()
            .map(|(id, record)| {
                id.parse::<u32>()
                    .map(|id| (id, Bytes::from(record)))
                    .map_err(|_| {
                        wacore::store::error::StoreError::Validation(format!(
                            "pre-key store returned a non-numeric id: {id}"
                        ))
                    })
            })
            .collect()
    }

    async fn remove_prekey(&self, id: u32) -> Result<()> {
        self.js_delete(STORE_PREKEY, &id.to_string()).await
    }

    async fn mark_prekeys_uploaded(&self, _ids: &[u32]) -> Result<()> {
        // Like InMemoryBackend: no per-row uploaded flag (store_prekey ignores
        // it); the upload window lives in the Device watermarks. The contract
        // that matters — never resurrecting deleted rows — holds trivially.
        Ok(())
    }

    async fn get_max_prekey_id(&self) -> Result<u32> {
        match self.js_get(STORE_META, META_MAX_PREKEY_ID).await? {
            Some(bytes) => {
                let s = String::from_utf8(bytes).unwrap_or_default();
                Ok(s.parse::<u32>().unwrap_or(0))
            }
            None => Ok(0),
        }
    }

    async fn store_prekeys_batch(&self, keys: &[(u32, Bytes)], uploaded: bool) -> Result<()> {
        let _ = uploaded;
        let entries = keys
            .iter()
            .map(|(id, record)| (id.to_string(), record.to_vec()))
            .collect::<Vec<_>>();
        self.js_put_many_owned(STORE_PREKEY, entries).await?;
        let max_id = keys
            .iter()
            .map(|(id, _)| *id)
            .max()
            .unwrap_or_default()
            .max(self.get_max_prekey_id().await?);
        self.js_set(
            STORE_META,
            META_MAX_PREKEY_ID,
            max_id.to_string().as_bytes(),
        )
        .await?;
        Ok(())
    }

    async fn store_signed_prekey(&self, id: u32, record: &[u8]) -> Result<()> {
        self.js_set(STORE_SIGNED_PREKEY, &id.to_string(), record)
            .await?;
        if self.needs_self_index() {
            let mut ids = self.get_signed_prekey_ids().await?;
            if !ids.contains(&id) {
                ids.push(id);
                self.js_set_json(STORE_META, META_SIGNED_PREKEY_IDS, &ids)
                    .await?;
            }
        }
        Ok(())
    }

    async fn load_signed_prekey(&self, id: u32) -> Result<Option<Vec<u8>>> {
        self.js_get(STORE_SIGNED_PREKEY, &id.to_string()).await
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
        self.js_delete(STORE_SIGNED_PREKEY, &id.to_string()).await?;
        if self.needs_self_index() {
            let mut ids = self.get_signed_prekey_ids().await?;
            ids.retain(|&i| i != id);
            self.js_set_json(STORE_META, META_SIGNED_PREKEY_IDS, &ids)
                .await?;
        }
        Ok(())
    }

    async fn put_sender_key(&self, address: &str, record: &[u8]) -> Result<()> {
        self.js_set(STORE_SENDER_KEY, address, record).await
    }

    async fn put_sender_keys_batch(&self, sender_keys: &[(Arc<str>, Bytes)]) -> Result<()> {
        let entries = sender_keys
            .iter()
            .map(|(address, record)| (address.to_string(), record.to_vec()))
            .collect::<Vec<_>>();
        self.js_put_many_owned(STORE_SENDER_KEY, entries).await
    }

    async fn get_sender_key(&self, address: &str) -> Result<Option<Vec<u8>>> {
        self.js_get(STORE_SENDER_KEY, address).await
    }

    async fn delete_sender_key(&self, address: &str) -> Result<()> {
        self.js_delete(STORE_SENDER_KEY, address).await
    }
}

impl JsBackend {
    async fn get_signed_prekey_ids(&self) -> Result<Vec<u32>> {
        if self.has_enumerate {
            // Derive ids from the store's keys (id.to_string()) directly.
            let keys = self.js_list_keys(STORE_SIGNED_PREKEY, None).await?;
            Ok(keys.iter().filter_map(|k| k.parse::<u32>().ok()).collect())
        } else {
            Ok(self
                .js_get_json::<Vec<u32>>(STORE_META, META_SIGNED_PREKEY_IDS)
                .await?
                .unwrap_or_default())
        }
    }
}

// ---------------------------------------------------------------------------
// AppSyncStore
// ---------------------------------------------------------------------------

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AppSyncStore for JsBackend {
    async fn get_sync_key(&self, key_id: &[u8]) -> Result<Option<AppStateSyncKey>> {
        let hex_id = to_hex(key_id);
        self.js_get_json(STORE_SYNC_KEY, &hex_id).await
    }

    async fn set_sync_key(&self, key_id: &[u8], key: AppStateSyncKey) -> Result<()> {
        let hex_id = to_hex(key_id);
        self.js_set_json(STORE_SYNC_KEY, &hex_id, &key).await?;
        self.js_set(STORE_META, META_LATEST_SYNC_KEY_ID, key_id)
            .await
    }

    async fn get_version(&self, name: &str) -> Result<HashState> {
        Ok(self
            .js_get_json(STORE_SYNC_VERSION, name)
            .await?
            .unwrap_or_default())
    }

    async fn set_version(&self, name: &str, state: HashState) -> Result<()> {
        self.js_set_json(STORE_SYNC_VERSION, name, &state).await
    }

    async fn put_mutation_macs(
        &self,
        name: &str,
        _version: u64,
        mutations: &[AppStateMutationMAC],
    ) -> Result<()> {
        let mut keys = Vec::with_capacity(mutations.len());
        for m in mutations {
            let key = format!("{}:{}", name, to_hex(&m.index_mac));
            self.js_set(STORE_MUTATION_MAC, &key, &m.value_mac).await?;
            keys.push(key);
        }
        // Track the keys so clear_mutation_macs can enumerate them on hosts that
        // can't list a namespace (mirrors sender_key_groups).
        if self.needs_self_index() && !keys.is_empty() {
            let mut idx: Vec<String> = self
                .js_get_json(STORE_META, MUTATION_MAC_INDEX)
                .await?
                .unwrap_or_default();
            let seen: std::collections::HashSet<&str> = idx.iter().map(String::as_str).collect();
            let mut fresh: Vec<String> = Vec::new();
            for key in keys {
                // Linear intra-batch dedup: `fresh` is batch-sized, and this
                // avoids a second owned HashSet of the whole index.
                if !seen.contains(key.as_str()) && !fresh.contains(&key) {
                    fresh.push(key);
                }
            }
            drop(seen);
            if !fresh.is_empty() {
                idx.extend(fresh);
                self.js_set_json(STORE_META, MUTATION_MAC_INDEX, &idx)
                    .await?;
            }
        }
        Ok(())
    }

    async fn get_mutation_mac(&self, name: &str, index_mac: &[u8]) -> Result<Option<Vec<u8>>> {
        let key = format!("{}:{}", name, to_hex(index_mac));
        self.js_get(STORE_MUTATION_MAC, &key).await
    }

    async fn delete_mutation_macs(&self, name: &str, index_macs: &[Vec<u8>]) -> Result<()> {
        for im in index_macs {
            let key = format!("{}:{}", name, to_hex(im));
            self.js_delete(STORE_MUTATION_MAC, &key).await?;
        }
        // Keep the self-index in step with the deletions.
        if self.needs_self_index() && !index_macs.is_empty() {
            let mut idx: Vec<String> = self
                .js_get_json(STORE_META, MUTATION_MAC_INDEX)
                .await?
                .unwrap_or_default();
            if !idx.is_empty() {
                let removing: std::collections::HashSet<String> = index_macs
                    .iter()
                    .map(|im| format!("{}:{}", name, to_hex(im)))
                    .collect();
                let before = idx.len();
                idx.retain(|k| !removing.contains(k));
                if idx.len() != before {
                    self.js_set_json(STORE_META, MUTATION_MAC_INDEX, &idx)
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn clear_mutation_macs(&self, name: &str) -> Result<()> {
        // Drop every mutation MAC for this collection on snapshot re-sync so the MAC
        // store is rebuilt from the snapshot. Keys are "<name>:<hex(index_mac)>"; the
        // trailing ':' scopes deletion to this exact collection (so "regular" can't
        // also wipe "regular_high"). Mirrors clear_all_sender_key_devices: a native
        // prefix delete when the host has one, else enumerate (listKeys on enumerate
        // hosts, the self-index otherwise) and delete the collection's keys.
        // Known migration gap: on self-index hosts, MACs stored before the index
        // existed are invisible here (they were never cleared pre-#766 either).
        let prefix = format!("{name}:");
        if !(self.has_prefix_delete
            && self
                .js_delete_prefix(STORE_MUTATION_MAC, &prefix)
                .await?
                .is_some())
        {
            let keys: Vec<String> = self
                .all_keys(STORE_MUTATION_MAC, MUTATION_MAC_INDEX)
                .await?
                .into_iter()
                .filter(|k| k.starts_with(&prefix))
                .collect();
            if !keys.is_empty() && !self.js_delete_many(STORE_MUTATION_MAC, &keys).await? {
                for k in &keys {
                    self.js_delete(STORE_MUTATION_MAC, k).await?;
                }
            }
        }
        // Drop the cleared keys from the self-index; delete the meta key when
        // nothing remains (mirrors clear_all_sender_key_devices), skip the
        // write when nothing matched.
        if self.needs_self_index() {
            let idx: Vec<String> = self
                .js_get_json(STORE_META, MUTATION_MAC_INDEX)
                .await?
                .unwrap_or_default();
            let before = idx.len();
            let remaining: Vec<String> = idx
                .into_iter()
                .filter(|k| !k.starts_with(&prefix))
                .collect();
            if remaining.is_empty() {
                if before != 0 {
                    self.js_delete(STORE_META, MUTATION_MAC_INDEX).await?;
                }
            } else if remaining.len() != before {
                self.js_set_json(STORE_META, MUTATION_MAC_INDEX, &remaining)
                    .await?;
            }
        }
        Ok(())
    }

    async fn get_latest_sync_key_id(&self) -> Result<Option<Vec<u8>>> {
        self.js_get(STORE_META, META_LATEST_SYNC_KEY_ID).await
    }
}

// ---------------------------------------------------------------------------
// ProtocolStore
// ---------------------------------------------------------------------------

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProtocolStore for JsBackend {
    // --- Sender Key Device Tracking ---

    async fn get_sender_key_devices(&self, group_jid: &str) -> Result<Vec<(String, bool)>> {
        Ok(self
            .js_get_json(STORE_SENDER_KEY_DEVICES, group_jid)
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
        self.js_set_json(STORE_SENDER_KEY_DEVICES, group_jid, &devices)
            .await?;
        // Track group JID so clear_all can enumerate them — only needed when the
        // host can't list the STORE_SENDER_KEY_DEVICES namespace itself.
        if self.needs_self_index() {
            let mut groups: Vec<String> = self
                .js_get_json(STORE_META, META_SENDER_KEY_GROUPS)
                .await?
                .unwrap_or_default();
            if !groups.iter().any(|g| g == group_jid) {
                groups.push(group_jid.to_string());
                self.js_set_json(STORE_META, META_SENDER_KEY_GROUPS, &groups)
                    .await?;
            }
        }
        Ok(())
    }

    async fn clear_sender_key_devices(&self, group_jid: &str) -> Result<()> {
        self.js_delete(STORE_SENDER_KEY_DEVICES, group_jid).await?;
        if self.needs_self_index() {
            // Remove from tracking list
            let mut groups: Vec<String> = self
                .js_get_json(STORE_META, META_SENDER_KEY_GROUPS)
                .await?
                .unwrap_or_default();
            if let Some(pos) = groups.iter().position(|g| g == group_jid) {
                groups.swap_remove(pos);
                self.js_set_json(STORE_META, META_SENDER_KEY_GROUPS, &groups)
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
            .all_keys(STORE_SENDER_KEY_DEVICES, META_SENDER_KEY_GROUPS)
            .await?;
        for group in &groups {
            let mut devices: Vec<(String, bool)> = self
                .js_get_json(STORE_SENDER_KEY_DEVICES, group)
                .await?
                .unwrap_or_default();
            let before = devices.len();
            devices.retain(|(jid, _)| !targets.contains(jid.as_str()));
            if devices.len() != before {
                self.js_set_json(STORE_SENDER_KEY_DEVICES, group, &devices)
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
                .js_delete_prefix(STORE_SENDER_KEY_DEVICES, ALL_KEYS_PREFIX)
                .await?
                .is_some())
        {
            let groups = self
                .all_keys(STORE_SENDER_KEY_DEVICES, META_SENDER_KEY_GROUPS)
                .await?;
            if !groups.is_empty()
                && !self
                    .js_delete_many(STORE_SENDER_KEY_DEVICES, &groups)
                    .await?
            {
                for group in &groups {
                    self.js_delete(STORE_SENDER_KEY_DEVICES, group).await?;
                }
            }
        }
        if self.needs_self_index() {
            self.js_delete(STORE_META, META_SENDER_KEY_GROUPS).await?;
        }
        Ok(())
    }

    // --- LID-PN Mapping ---

    async fn get_lid_mapping(&self, lid: &str) -> Result<Option<LidPnMappingEntry>> {
        self.js_get_json(STORE_LID_MAPPING, &format!("lid:{lid}"))
            .await
    }

    async fn get_pn_mapping(&self, phone: &str) -> Result<Option<LidPnMappingEntry>> {
        match self
            .js_get(STORE_LID_MAPPING, &format!("pn:{phone}"))
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
            self.js_delete(STORE_LID_MAPPING, &format!("pn:{}", old_entry.phone_number))
                .await?;
        }
        // Forward mapping (lid -> entry)
        self.js_set_json(STORE_LID_MAPPING, &format!("lid:{}", entry.lid), entry)
            .await?;
        // Reverse mapping (pn -> lid)
        self.js_set(
            STORE_LID_MAPPING,
            &format!("pn:{}", entry.phone_number),
            entry.lid.as_bytes(),
        )
        .await?;
        // `lid_list` tracks bare lids for get_all; only needed when the host
        // can't enumerate the `lid:`-prefixed keys itself.
        if self.needs_self_index() {
            let mut lids: Vec<String> = self
                .js_get_json(STORE_META, META_LID_LIST)
                .await?
                .unwrap_or_default();
            if !lids.contains(&entry.lid) {
                lids.push(entry.lid.clone());
                self.js_set_json(STORE_META, META_LID_LIST, &lids).await?;
            }
        }
        Ok(())
    }

    async fn get_all_lid_mappings(&self) -> Result<Vec<LidPnMappingEntry>> {
        // Enumerate the `lid:`-prefixed keys directly, or fall back to the
        // bare-lid index. Either way, resolve each via get_lid_mapping.
        let lids: Vec<String> = if self.has_enumerate {
            self.js_list_keys(STORE_LID_MAPPING, Some("lid:"))
                .await?
                .into_iter()
                .filter_map(|k| k.strip_prefix("lid:").map(str::to_string))
                .collect()
        } else {
            self.js_get_json(STORE_META, META_LID_LIST)
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
        self.js_set(STORE_BASE_KEY, &key, base_key).await
    }

    async fn has_same_base_key(
        &self,
        address: &str,
        message_id: &str,
        current_base_key: &[u8],
    ) -> Result<bool> {
        let key = format!("{address}:{message_id}");
        match self.js_get(STORE_BASE_KEY, &key).await? {
            Some(stored) => Ok(stored == current_base_key),
            None => Ok(false),
        }
    }

    async fn delete_base_key(&self, address: &str, message_id: &str) -> Result<()> {
        let key = format!("{address}:{message_id}");
        self.js_delete(STORE_BASE_KEY, &key).await
    }

    // --- Device Registry ---

    async fn update_device_list(&self, record: DeviceListRecord) -> Result<()> {
        self.js_set_json(STORE_DEVICE_LIST, &record.user, &record)
            .await
    }

    async fn get_devices(&self, user: &str) -> Result<Option<DeviceListRecord>> {
        self.js_get_json(STORE_DEVICE_LIST, user).await
    }

    async fn delete_devices(&self, user: &str) -> Result<()> {
        self.js_delete(STORE_DEVICE_LIST, user).await
    }

    // --- TcToken Storage ---

    async fn get_tc_token(&self, jid: &str) -> Result<Option<TcTokenEntry>> {
        self.js_get_json(STORE_TC_TOKEN, jid).await
    }

    async fn put_tc_token(&self, jid: &str, entry: &TcTokenEntry) -> Result<()> {
        self.js_set_json(STORE_TC_TOKEN, jid, entry).await?;
        let mut jids: Vec<String> = self
            .js_get_json(STORE_META, META_TC_TOKEN_JIDS)
            .await?
            .unwrap_or_default();
        if !jids.iter().any(|j| j == jid) {
            jids.push(jid.to_string());
            self.js_set_json(STORE_META, META_TC_TOKEN_JIDS, &jids)
                .await?;
        }
        Ok(())
    }

    async fn delete_tc_token(&self, jid: &str) -> Result<()> {
        self.js_delete(STORE_TC_TOKEN, jid).await?;
        let mut jids: Vec<String> = self
            .js_get_json(STORE_META, META_TC_TOKEN_JIDS)
            .await?
            .unwrap_or_default();
        jids.retain(|j| j != jid);
        self.js_set_json(STORE_META, META_TC_TOKEN_JIDS, &jids)
            .await
    }

    async fn get_all_tc_token_jids(&self) -> Result<Vec<String>> {
        Ok(self
            .js_get_json(STORE_META, META_TC_TOKEN_JIDS)
            .await?
            .unwrap_or_default())
    }

    async fn delete_expired_tc_tokens(&self, token_cutoff: i64, sender_cutoff: i64) -> Result<u32> {
        let jids = self.get_all_tc_token_jids().await?;
        let mut deleted = 0u32;
        let mut remaining_jids = Vec::new();
        for jid in jids {
            if let Some(entry) = self
                .js_get_json::<TcTokenEntry>(STORE_TC_TOKEN, &jid)
                .await?
            {
                let token_live = !entry.token.is_empty() && entry.token_timestamp >= token_cutoff;
                let sender_live = entry
                    .sender_timestamp
                    .is_some_and(|timestamp| timestamp >= sender_cutoff);
                if !token_live && !sender_live {
                    self.js_delete(STORE_TC_TOKEN, &jid).await?;
                    deleted += 1;
                } else {
                    remaining_jids.push(jid);
                }
            }
        }
        self.js_set_json(STORE_META, META_TC_TOKEN_JIDS, &remaining_jids)
            .await?;
        Ok(deleted)
    }

    // --- Sent Message Store ---

    #[cfg_attr(
        feature = "memory-profiling",
        tracing::instrument(name = "bridge.store.sent_message", level = "trace", skip_all)
    )]
    async fn store_sent_message(
        &self,
        chat_jid: &str,
        message_id: &str,
        payload: &[u8],
    ) -> Result<()> {
        let key = compound_store_key([chat_jid, message_id]);
        let now = wacore::time::now_secs();
        let mut data = Vec::with_capacity(TIMESTAMP_PREFIX_LEN + payload.len());
        data.extend_from_slice(&now.to_be_bytes());
        data.extend_from_slice(payload);
        self.js_set(STORE_SENT_MESSAGE, &key, &data).await?;

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
        let key = compound_store_key([chat_jid, message_id]);

        // Fetch and delete from store WITHOUT holding the mutex
        let data = match self.js_get(STORE_SENT_MESSAGE, &key).await? {
            Some(data) if data.len() > TIMESTAMP_PREFIX_LEN => data,
            _ => return Ok(None),
        };
        self.js_delete(STORE_SENT_MESSAGE, &key).await?;

        // Brief lock to update in-memory index (self-index path only).
        if self.needs_self_index() {
            let mut guard = self.sent_message_keys.lock().await;
            if let Some(ref mut keys) = *guard {
                keys.retain(|k| k != &key);
            }
        }

        // Skip the timestamp prefix.
        Ok(Some(data[TIMESTAMP_PREFIX_LEN..].to_vec()))
    }

    async fn delete_expired_sent_messages(&self, cutoff_timestamp: i64) -> Result<u32> {
        // Enumerate-capable hosts: list the namespace and scan in bounded
        // chunks — no in-memory index to maintain.
        if self.has_enumerate {
            let keys = self.js_list_keys(STORE_SENT_MESSAGE, None).await?;
            let (victims, _survivors) = self
                .scan_expired(STORE_SENT_MESSAGE, &keys, cutoff_timestamp)
                .await?;
            // Re-validate right before delete so a concurrently-rewritten entry
            // (fresh timestamp) isn't deleted. No self-index on this path, so
            // we only need the still-expired set.
            let (victims, _revived) = self
                .confirm_expired(STORE_SENT_MESSAGE, victims, cutoff_timestamp)
                .await?;
            if !victims.is_empty() && !self.js_delete_many(STORE_SENT_MESSAGE, &victims).await? {
                for key in &victims {
                    self.js_delete(STORE_SENT_MESSAGE, key).await?;
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
            match self.js_get(STORE_SENT_MESSAGE, key).await {
                Ok(Some(data)) if data.len() >= TIMESTAMP_PREFIX_LEN => {
                    let ts = i64::from_be_bytes(
                        data[..TIMESTAMP_PREFIX_LEN]
                            .try_into()
                            .unwrap_or([0; TIMESTAMP_PREFIX_LEN]),
                    );
                    if ts < cutoff_timestamp {
                        self.js_delete(STORE_SENT_MESSAGE, key).await?;
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
                    self.js_delete(STORE_SENT_MESSAGE, key).await?;
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
// MsgSecretStore
// ---------------------------------------------------------------------------

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MsgSecretStore for JsBackend {
    async fn put_msg_secret(
        &self,
        chat: &str,
        sender: &str,
        msg_id: &str,
        secret: &[u8; wacore::reporting_token::MESSAGE_SECRET_SIZE],
    ) -> Result<()> {
        let key = compound_store_key([chat, sender, msg_id]);
        // The BE timestamp prefix powers delete_expired_msg_secrets.
        let now = wacore::time::now_secs();
        let mut data = Vec::with_capacity(TIMESTAMP_PREFIX_LEN + secret.len());
        data.extend_from_slice(&now.to_be_bytes());
        data.extend_from_slice(secret);
        self.js_set(STORE_MSG_SECRET, &key, &data).await?;

        // Self-index only when the host can't enumerate; an enumerate-capable
        // host derives the key set from the store directly (see all_keys).
        if self.needs_self_index() {
            let mut keys: Vec<String> = self
                .js_get_json(STORE_META, META_MSG_SECRET_KEYS)
                .await?
                .unwrap_or_default();
            if !keys.iter().any(|k| k == &key) {
                keys.push(key);
                self.js_set_json(STORE_META, META_MSG_SECRET_KEYS, &keys)
                    .await?;
            }
        }
        Ok(())
    }

    /// Batched variant. Builds the `[(key, value)]` batch once and writes it in
    /// a single `setMany` crossing (history sync delivers tens of thousands of
    /// secrets at once). The self-index is only maintained for non-enumerable
    /// hosts, and even then loaded/rewritten ONCE (HashSet dedupe → O(n), not
    /// the O(n²) a naive per-entry read+rewrite would cost).
    #[cfg_attr(
        feature = "memory-profiling",
        tracing::instrument(name = "bridge.store.msg_secrets", level = "trace", skip_all)
    )]
    async fn put_msg_secrets(&self, entries: Vec<MsgSecretEntry>) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        // Single timestamp for the batch; the BE prefix powers
        // delete_expired_msg_secrets (same format as put_msg_secret).
        let now_bytes = wacore::time::now_secs().to_be_bytes();
        let mut batch: Vec<(String, Vec<u8>)> = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = compound_store_key([
                entry.chat.as_ref(),
                entry.sender.as_ref(),
                entry.msg_id.as_ref(),
            ]);
            let mut data = Vec::with_capacity(TIMESTAMP_PREFIX_LEN + entry.secret.len());
            data.extend_from_slice(&now_bytes);
            data.extend_from_slice(entry.secret.as_ref());
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
                .js_get_json(STORE_META, META_MSG_SECRET_KEYS)
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
                self.js_set_json(STORE_META, META_MSG_SECRET_KEYS, &keys)
                    .await?;
            }
        }

        self.js_put_many_owned(STORE_MSG_SECRET, batch).await?;
        Ok(stored)
    }

    async fn get_msg_secret(
        &self,
        chat: &str,
        sender: &str,
        msg_id: &str,
    ) -> Result<Option<Vec<u8>>> {
        let key = compound_store_key([chat, sender, msg_id]);
        // Strip the timestamp prefix written by put_msg_secret.
        Ok(self
            .js_get(STORE_MSG_SECRET, &key)
            .await?
            .filter(|d| d.len() >= TIMESTAMP_PREFIX_LEN)
            .map(|d| d[TIMESTAMP_PREFIX_LEN..].to_vec()))
    }

    async fn delete_expired_msg_secrets(&self, cutoff_timestamp: i64) -> Result<u32> {
        // Key set from enumeration (enumerate hosts) or the JSON index (legacy
        // hosts) — same code path either way, satisfying "legacy still expires
        // via self-index".
        let keys = self
            .all_keys(STORE_MSG_SECRET, META_MSG_SECRET_KEYS)
            .await?;
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

        if !victims.is_empty() && !self.js_delete_many(STORE_MSG_SECRET, &victims).await? {
            for key in &victims {
                self.js_delete(STORE_MSG_SECRET, key).await?;
            }
        }

        // Rewrite the index to the survivors only on the self-index path, and
        // only if anything was actually removed (deleted or vanished).
        if self.needs_self_index() && survivors.len() != keys.len() {
            self.js_set_json(STORE_META, META_MSG_SECRET_KEYS, &survivors)
                .await?;
        }
        Ok(victims.len() as u32)
    }
}

// ---------------------------------------------------------------------------
// DeviceStore
// ---------------------------------------------------------------------------

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl DeviceStore for JsBackend {
    async fn save(&self, device: &Device) -> Result<()> {
        self.js_set_json(STORE_DEVICE, DEVICE_RECORD, device)
            .await?;

        // `account` (AdvSignedDeviceIdentity) is #[serde(skip)] in Device,
        // so we persist it separately as raw protobuf bytes — same approach
        // as SQLite storage which uses a dedicated column.
        if let Some(ref account) = device.account {
            self.js_set(STORE_DEVICE, DEVICE_ACCOUNT, &account.encode_to_vec())
                .await?;
        }

        Ok(())
    }

    async fn load(&self) -> Result<Option<Device>> {
        let mut device: Option<Device> = self.js_get_json(STORE_DEVICE, DEVICE_RECORD).await?;

        // Restore the #[serde(skip)] `account` field from its separate key.
        if let Some(ref mut dev) = device
            && let Some(bytes) = self.js_get(STORE_DEVICE, DEVICE_ACCOUNT).await?
        {
            match waproto::whatsapp::ADVSignedDeviceIdentity::decode_from_slice(bytes.as_slice()) {
                Ok(account) => dev.account = Some(account.into()),
                Err(e) => log::warn!("Failed to decode stored account identity: {e}"),
            }
        }

        Ok(device)
    }

    async fn exists(&self) -> Result<bool> {
        Ok(self.js_get(STORE_DEVICE, DEVICE_RECORD).await?.is_some())
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
// Helpers
// ---------------------------------------------------------------------------

/// Parse a JS `Array<[string, Uint8Array]>` (the shape returned by `getMany`)
/// into owned `(key, value)` pairs. STRICT: a malformed tuple FAILS the whole
/// batch rather than being silently dropped — a dropped entry would look like a
/// missing key to the caller, which on the expiry path can prune a live key
/// from the self-index while its value is still stored.
fn parse_entry_array(value: JsValue, context: &'static str) -> Result<Vec<(String, Vec<u8>)>> {
    let arr: js_sys::Array = value
        .dyn_into()
        .map_err(|_| js_err_to_store_err(context, JsValue::from_str("expected array")))?;
    let malformed =
        || js_err_to_store_err(context, JsValue::from_str("malformed [key, value] entry"));
    let mut out = Vec::with_capacity(arr.length() as usize);
    for entry in arr.iter() {
        let tuple = entry.dyn_into::<js_sys::Array>().map_err(|_| malformed())?;
        let key = tuple.get(0).as_string().ok_or_else(malformed)?;
        let val = tuple
            .get(1)
            .dyn_into::<Uint8Array>()
            .map_err(|_| malformed())?;
        out.push((key, val.to_vec()));
    }
    Ok(out)
}

async fn resolve_promise(value: JsValue) -> std::result::Result<JsValue, JsValue> {
    if value.is_instance_of::<Promise>() {
        JsFuture::from(Promise::unchecked_from_js(value)).await
    } else {
        Ok(value)
    }
}

/// Errors raised by the JS-provided storage callbacks (`get`/`set`/`delete`).
/// Distinct from `serde_json` (de)serialization failures and from validation
/// errors so consumers can downcast off `StoreError::Database`'s source chain
/// to discriminate.
#[derive(Debug, thiserror::Error)]
#[error("JS {context}: {message}")]
pub(crate) struct JsCallbackError {
    pub context: &'static str,
    pub message: String,
}

/// Wraps a `serde_json` (de)serialization failure with the `<store>/<key>`
/// context. Source chain preserves the original `serde_json::Error` for
/// downcast.
#[derive(Debug, thiserror::Error)]
#[error("{op} {store}/{key}")]
pub(crate) struct JsonStoreError {
    pub op: &'static str,
    pub store: String,
    pub key: String,
    #[source]
    pub source: serde_json::Error,
}

fn js_err_to_store_err(context: &'static str, e: JsValue) -> wacore::store::error::StoreError {
    // Rejected promises normally carry an Error object, not a JS string.
    // `JsValue::as_string()` therefore returns `None` for the useful case and
    // the old fallback reduced the callback failure to an opaque debug value.
    // Reading `.message` keeps the original host/codec reason in the Rust
    // source chain without retaining a stack or adding work to the success path.
    let message = e
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(&e, &JsValue::from_str("message"))
                .ok()
                .and_then(|value| value.as_string())
        })
        .unwrap_or_else(|| format!("{e:?}"));
    wacore::store::error::StoreError::Database(Box::new(JsCallbackError { context, message }))
}

#[cfg(test)]
mod js_callback_error_tests {
    use super::*;
    use std::error::Error as _;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn rejected_error_object_preserves_its_message_in_the_source_chain() {
        let callback_error = js_sys::Error::new("session projection failed");
        let store_error = js_err_to_store_err("setMany", callback_error.into());

        assert_eq!(
            store_error.source().map(ToString::to_string).as_deref(),
            Some("JS setMany: session projection failed")
        );
    }
}

/// Simple hex encoding for byte slices (avoids adding `hex` crate dependency).
fn to_hex(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0xf) as usize] as char);
    }
    s
}
