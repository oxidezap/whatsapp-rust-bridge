//! `HostCacheStore` over a JS `JsCacheStore` (get/set/delete/clear) + parsing of
//! the JS `CacheConfig` object into a `bridge_core::cache::CacheSpec`. The
//! `CacheConfig` assembly itself lives in bridge-core (engine-agnostic).

use std::sync::Arc;

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use wacore::store::cache::CacheStore;

use bridge_core::backend::new_core_cache_store;
use bridge_core::cache::{CacheEntrySpec, CacheSpec};
use bridge_core::host::HostCacheStore;

type Tsfn<I, O> = ThreadsafeFunction<I, Promise<O>, I, Status, false>;
type CacheGetTsfn = Tsfn<FnArgs<(String, String)>, Option<Uint8Array>>;
type CacheSetTsfn = Tsfn<FnArgs<(String, String, Uint8Array, Option<f64>)>, ()>;
type CacheDeleteTsfn = Tsfn<FnArgs<(String, String)>, ()>;
type CacheClearTsfn = Tsfn<String, ()>;

pub struct NapiCacheStore {
    get: Arc<CacheGetTsfn>,
    set: Arc<CacheSetTsfn>,
    delete: Arc<CacheDeleteTsfn>,
    clear: Arc<CacheClearTsfn>,
}

fn anyhow_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

#[async_trait]
impl HostCacheStore for NapiCacheStore {
    async fn get(&self, ns: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let p = self
            .get
            .call_async(FnArgs::from((ns.to_string(), key.to_string())))
            .await
            .map_err(anyhow_err)?;
        Ok(p.await.map_err(anyhow_err)?.map(|u| u.to_vec()))
    }

    async fn set(
        &self,
        ns: &str,
        key: &str,
        value: &[u8],
        ttl_secs: Option<f64>,
    ) -> anyhow::Result<()> {
        let args = FnArgs::from((
            ns.to_string(),
            key.to_string(),
            Uint8Array::from(value.to_vec()),
            ttl_secs,
        ));
        let p = self.set.call_async(args).await.map_err(anyhow_err)?;
        p.await.map_err(anyhow_err)?;
        Ok(())
    }

    async fn delete(&self, ns: &str, key: &str) -> anyhow::Result<()> {
        let p = self
            .delete
            .call_async(FnArgs::from((ns.to_string(), key.to_string())))
            .await
            .map_err(anyhow_err)?;
        p.await.map_err(anyhow_err)?;
        Ok(())
    }

    async fn clear(&self, ns: &str) -> anyhow::Result<()> {
        let p = self
            .clear
            .call_async(ns.to_string())
            .await
            .map_err(anyhow_err)?;
        p.await.map_err(anyhow_err)?;
        Ok(())
    }
}

/// Build a `CacheStore` from a JS `JsCacheStore` object (get/set/delete/clear).
fn build_cache_store(env: &Env, obj: &Object) -> Result<Arc<dyn CacheStore>> {
    let mut get = obj
        .get_named_property::<Function<FnArgs<(String, String)>, Promise<Option<Uint8Array>>>>(
            "get",
        )?
        .build_threadsafe_function()
        .build()?;
    get.unref(env)?;
    let mut set = obj
        .get_named_property::<Function<FnArgs<(String, String, Uint8Array, Option<f64>)>, Promise<()>>>(
            "set",
        )?
        .build_threadsafe_function()
        .build()?;
    set.unref(env)?;
    let mut delete = obj
        .get_named_property::<Function<FnArgs<(String, String)>, Promise<()>>>("delete")?
        .build_threadsafe_function()
        .build()?;
    delete.unref(env)?;
    let mut clear = obj
        .get_named_property::<Function<String, Promise<()>>>("clear")?
        .build_threadsafe_function()
        .build()?;
    clear.unref(env)?;

    Ok(new_core_cache_store(Arc::new(NapiCacheStore {
        get: Arc::new(get),
        set: Arc::new(set),
        delete: Arc::new(delete),
        clear: Arc::new(clear),
    })))
}

/// Parse a JS `CacheConfig` object into a `CacheSpec` (numbers + pluggable stores).
pub fn build_cache_spec(env: &Env, obj: &Object) -> Result<CacheSpec> {
    let mut spec = CacheSpec::default();

    if let Ok(store_obj) = obj.get_named_property::<Object>("store") {
        spec.global_store = Some(build_cache_store(env, &store_obj)?);
    }
    spec.group = entry_spec(env, obj, "group", true)?;
    spec.device_registry = entry_spec(env, obj, "deviceRegistry", true)?;
    spec.lid_pn = entry_spec(env, obj, "lidPn", true)?;
    spec.recent_messages = entry_spec(env, obj, "recentMessages", false)?;
    spec.message_retry = entry_spec(env, obj, "messageRetry", false)?;

    Ok(spec)
}

fn entry_spec(env: &Env, parent: &Object, key: &str, with_store: bool) -> Result<CacheEntrySpec> {
    let mut spec = CacheEntrySpec::default();
    let Ok(obj) = parent.get_named_property::<Object>(key) else {
        return Ok(spec);
    };
    spec.ttl_secs = obj.get_named_property::<f64>("ttlSecs").ok();
    spec.capacity = obj
        .get_named_property::<f64>("capacity")
        .ok()
        .map(|c| c as u64);
    if with_store && let Ok(store_obj) = obj.get_named_property::<Object>("store") {
        spec.store = Some(build_cache_store(env, &store_obj)?);
    }
    Ok(spec)
}
