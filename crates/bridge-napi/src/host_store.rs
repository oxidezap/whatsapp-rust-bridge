//! `HostStore` over the JS `JsStoreCallbacks` (get/set/delete + optional
//! batch/enumerate/prefix), each a CalleeHandled=false TSFN. The heavy store
//! logic (write-back, self-index, expiry, the 6 wacore traits) lives in
//! `bridge_core::backend::CoreBackend`, which drives this seam.

use std::sync::Arc;

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;

use bridge_core::host::{HostStore, HostStoreCapabilities};

type Tsfn<I, O> = ThreadsafeFunction<I, Promise<O>, I, Status, false>;

pub type GetTsfn = Tsfn<FnArgs<(String, String)>, Option<Uint8Array>>;
pub type SetTsfn = Tsfn<FnArgs<(String, String, Uint8Array)>, ()>;
pub type DeleteTsfn = Tsfn<FnArgs<(String, String)>, ()>;
pub type SetManyTsfn = Tsfn<FnArgs<(String, Vec<(String, Uint8Array)>)>, ()>;
pub type DeleteManyTsfn = Tsfn<FnArgs<(String, Vec<String>)>, ()>;
pub type GetManyTsfn = Tsfn<FnArgs<(String, Vec<String>)>, Vec<(String, Uint8Array)>>;
pub type ListKeysTsfn = Tsfn<FnArgs<(String, Option<String>)>, Vec<String>>;
pub type DeletePrefixTsfn = Tsfn<FnArgs<(String, String)>, f64>;

pub struct NapiStore {
    pub get: Arc<GetTsfn>,
    pub set: Arc<SetTsfn>,
    pub delete: Arc<DeleteTsfn>,
    pub set_many: Option<Arc<SetManyTsfn>>,
    pub delete_many: Option<Arc<DeleteManyTsfn>>,
    pub get_many: Option<Arc<GetManyTsfn>>,
    pub list_keys: Option<Arc<ListKeysTsfn>>,
    pub delete_prefix: Option<Arc<DeletePrefixTsfn>>,
    pub caps: HostStoreCapabilities,
}

fn anyhow_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

#[async_trait]
impl HostStore for NapiStore {
    async fn get(&self, store: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let args = FnArgs::from((store.to_string(), key.to_string()));
        let promise = self.get.call_async(args).await.map_err(anyhow_err)?;
        let value = promise.await.map_err(anyhow_err)?;
        Ok(value.map(|u| u.to_vec()))
    }

    async fn set(&self, store: &str, key: &str, value: &[u8]) -> anyhow::Result<()> {
        let args = FnArgs::from((
            store.to_string(),
            key.to_string(),
            Uint8Array::from(value.to_vec()),
        ));
        let promise = self.set.call_async(args).await.map_err(anyhow_err)?;
        promise.await.map_err(anyhow_err)?;
        Ok(())
    }

    async fn delete(&self, store: &str, key: &str) -> anyhow::Result<()> {
        let args = FnArgs::from((store.to_string(), key.to_string()));
        let promise = self.delete.call_async(args).await.map_err(anyhow_err)?;
        promise.await.map_err(anyhow_err)?;
        Ok(())
    }

    async fn set_many(&self, store: &str, entries: &[(String, Vec<u8>)]) -> anyhow::Result<bool> {
        let Some(tsfn) = &self.set_many else {
            return Ok(false);
        };
        let js_entries: Vec<(String, Uint8Array)> = entries
            .iter()
            .map(|(k, v)| (k.clone(), Uint8Array::from(v.clone())))
            .collect();
        let args = FnArgs::from((store.to_string(), js_entries));
        let promise = tsfn.call_async(args).await.map_err(anyhow_err)?;
        promise.await.map_err(anyhow_err)?;
        Ok(true)
    }

    async fn delete_many(&self, store: &str, keys: &[String]) -> anyhow::Result<bool> {
        let Some(tsfn) = &self.delete_many else {
            return Ok(false);
        };
        let args = FnArgs::from((store.to_string(), keys.to_vec()));
        let promise = tsfn.call_async(args).await.map_err(anyhow_err)?;
        promise.await.map_err(anyhow_err)?;
        Ok(true)
    }

    async fn get_many(
        &self,
        store: &str,
        keys: &[String],
    ) -> anyhow::Result<Option<Vec<(String, Vec<u8>)>>> {
        let Some(tsfn) = &self.get_many else {
            return Ok(None);
        };
        let args = FnArgs::from((store.to_string(), keys.to_vec()));
        let promise = tsfn.call_async(args).await.map_err(anyhow_err)?;
        let entries = promise.await.map_err(anyhow_err)?;
        Ok(Some(
            entries.into_iter().map(|(k, v)| (k, v.to_vec())).collect(),
        ))
    }

    async fn list_keys(&self, store: &str, prefix: Option<&str>) -> anyhow::Result<Vec<String>> {
        let Some(tsfn) = &self.list_keys else {
            return Ok(Vec::new());
        };
        let args = FnArgs::from((store.to_string(), prefix.map(|s| s.to_string())));
        let promise = tsfn.call_async(args).await.map_err(anyhow_err)?;
        promise.await.map_err(anyhow_err)
    }

    async fn delete_prefix(&self, store: &str, prefix: &str) -> anyhow::Result<Option<u32>> {
        let Some(tsfn) = &self.delete_prefix else {
            return Ok(None);
        };
        let args = FnArgs::from((store.to_string(), prefix.to_string()));
        let promise = tsfn.call_async(args).await.map_err(anyhow_err)?;
        let n = promise.await.map_err(anyhow_err)?;
        Ok(Some(n as u32))
    }

    fn capabilities(&self) -> HostStoreCapabilities {
        self.caps
    }
}

/// Extract the JS `JsStoreCallbacks` methods into TSFNs (each `unref`'d) and the
/// declared capabilities. Runs synchronously on the JS thread in `createWhatsAppClient`.
pub fn build_napi_store(env: &Env, obj: &Object) -> Result<NapiStore> {
    let mut get = obj
        .get_named_property::<Function<FnArgs<(String, String)>, Promise<Option<Uint8Array>>>>(
            "get",
        )?
        .build_threadsafe_function()
        .build()?;
    get.unref(env)?;
    let mut set = obj
        .get_named_property::<Function<FnArgs<(String, String, Uint8Array)>, Promise<()>>>("set")?
        .build_threadsafe_function()
        .build()?;
    set.unref(env)?;
    let mut delete = obj
        .get_named_property::<Function<FnArgs<(String, String)>, Promise<()>>>("delete")?
        .build_threadsafe_function()
        .build()?;
    delete.unref(env)?;

    let set_many = match obj
        .get_named_property::<Function<FnArgs<(String, Vec<(String, Uint8Array)>)>, Promise<()>>>(
            "setMany",
        ) {
        Ok(f) => {
            let mut t = f.build_threadsafe_function().build()?;
            t.unref(env)?;
            Some(Arc::new(t))
        }
        Err(_) => None,
    };
    let delete_many = match obj
        .get_named_property::<Function<FnArgs<(String, Vec<String>)>, Promise<()>>>("deleteMany")
    {
        Ok(f) => {
            let mut t = f.build_threadsafe_function().build()?;
            t.unref(env)?;
            Some(Arc::new(t))
        }
        Err(_) => None,
    };
    let get_many = match obj
        .get_named_property::<Function<FnArgs<(String, Vec<String>)>, Promise<Vec<(String, Uint8Array)>>>>(
            "getMany",
        ) {
        Ok(f) => {
            let mut t = f.build_threadsafe_function().build()?;
            t.unref(env)?;
            Some(Arc::new(t))
        }
        Err(_) => None,
    };
    let list_keys = match obj
        .get_named_property::<Function<FnArgs<(String, Option<String>)>, Promise<Vec<String>>>>(
            "listKeys",
        ) {
        Ok(f) => {
            let mut t = f.build_threadsafe_function().build()?;
            t.unref(env)?;
            Some(Arc::new(t))
        }
        Err(_) => None,
    };
    let delete_prefix = match obj
        .get_named_property::<Function<FnArgs<(String, String)>, Promise<f64>>>("deletePrefix")
    {
        Ok(f) => {
            let mut t = f.build_threadsafe_function().build()?;
            t.unref(env)?;
            Some(Arc::new(t))
        }
        Err(_) => None,
    };

    let cap_obj = obj.get_named_property::<Object>("capabilities").ok();
    let read = |name: &str| -> bool {
        cap_obj
            .as_ref()
            .and_then(|c| c.get_named_property::<bool>(name).ok())
            .unwrap_or(false)
    };
    let caps = HostStoreCapabilities {
        batch: read("batch"),
        enumerate: read("enumerate"),
        prefix_delete: read("prefixDelete"),
        write_back: read("writeBack"),
    };

    Ok(NapiStore {
        get: Arc::new(get),
        set: Arc::new(set),
        delete: Arc::new(delete),
        set_many,
        delete_many,
        get_many,
        list_keys,
        delete_prefix,
        caps,
    })
}
