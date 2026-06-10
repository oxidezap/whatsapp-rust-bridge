//! Engine-agnostic assembly of `whatsapp_rust::CacheConfig` from a parsed spec
//! (TTLs/capacities + optional pluggable `CacheStore`s). Mirrors the old
//! `build_cache_config`/`apply_*` in `wasm_client.rs`; the engine binding only
//! parses the JS config object and builds the `CacheStore`s (NapiCacheStore).

use std::sync::Arc;
use std::time::Duration;

use wacore::store::cache::CacheStore;
use whatsapp_rust::{CacheConfig, CacheEntryConfig, CacheStores};

/// One cache entry's overrides. `store` (when set) replaces the backend for
/// just this cache (takes priority over the global store).
#[derive(Default)]
pub struct CacheEntrySpec {
    pub ttl_secs: Option<f64>,
    pub capacity: Option<u64>,
    pub store: Option<Arc<dyn CacheStore>>,
}

/// Parsed JS `CacheConfig`. The 5 entries map to the same config fields the old
/// `build_cache_config` touched; `recent_messages`/`message_retry` have no store.
#[derive(Default)]
pub struct CacheSpec {
    pub global_store: Option<Arc<dyn CacheStore>>,
    pub group: CacheEntrySpec,
    pub device_registry: CacheEntrySpec,
    pub lid_pn: CacheEntrySpec,
    pub recent_messages: CacheEntrySpec,
    pub message_retry: CacheEntrySpec,
}

pub fn build_cache_config(spec: CacheSpec) -> CacheConfig {
    let mut config = CacheConfig::default();

    if let Some(store) = &spec.global_store {
        config.cache_stores = CacheStores::all(store.clone());
    }

    apply_entry(
        &spec.group,
        &mut config.group_cache,
        &mut config.cache_stores.group_cache,
    );
    apply_entry(
        &spec.device_registry,
        &mut config.device_registry_cache,
        &mut config.cache_stores.device_registry_cache,
    );
    apply_entry(
        &spec.lid_pn,
        &mut config.lid_pn_cache,
        &mut config.cache_stores.lid_pn_cache,
    );
    apply_ttl_cap(&spec.recent_messages, &mut config.recent_messages);
    apply_ttl_cap(&spec.message_retry, &mut config.message_retry_counts);

    config
}

fn apply_entry(
    spec: &CacheEntrySpec,
    entry: &mut CacheEntryConfig,
    slot: &mut Option<Arc<dyn CacheStore>>,
) {
    apply_ttl_cap(spec, entry);
    if let Some(store) = &spec.store {
        *slot = Some(store.clone());
    }
}

fn apply_ttl_cap(spec: &CacheEntrySpec, entry: &mut CacheEntryConfig) {
    if let Some(secs) = spec.ttl_secs {
        // `0` (or negative) means "no expiry", matching the old `ttlSecs` semantics.
        entry.timeout = if secs > 0.0 {
            Some(Duration::from_secs(secs as u64))
        } else {
            None
        };
    }
    if let Some(cap) = spec.capacity {
        entry.capacity = cap;
    }
}
