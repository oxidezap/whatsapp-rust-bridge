//! Adapter that implements the Rust `CacheStore` trait by delegating to JS callbacks.
//!
//! This allows users to provide custom cache backends (Redis, Memcached, etc.)
//! from JavaScript while the Rust client uses them transparently.

use async_trait::async_trait;
use js_sys::{Promise, Uint8Array};
use std::time::Duration;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use whatsapp_rust::wacore::store::cache::CacheStore;

generated_cache_callbacks!();

crate::wasm_send_sync!(JsCacheStoreAdapter);

impl JsCacheStoreAdapter {
    /// Check if a JS value looks like a JsCacheStore (has a `get` function).
    pub fn is_cache_store(val: &JsValue) -> bool {
        js_sys::Reflect::get(val, &"get".into())
            .map(|v| v.is_function())
            .unwrap_or(false)
    }
}

async fn resolve_promise(val: JsValue) -> Result<JsValue, anyhow::Error> {
    if val.is_instance_of::<Promise>() {
        let promise = Promise::<JsValue>::unchecked_from_js(val);
        JsFuture::from(promise)
            .await
            .map_err(|e| anyhow::anyhow!("JS cache store error: {e:?}"))
    } else {
        Ok(val)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CacheStore for JsCacheStoreAdapter {
    async fn get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let result = self
            .get_fn
            .call2(
                &JsValue::NULL,
                &JsValue::from_str(namespace),
                &JsValue::from_str(key),
            )
            .map_err(|e| anyhow::anyhow!("cache get error: {e:?}"))?;

        let resolved = resolve_promise(result).await?;

        if resolved.is_null() || resolved.is_undefined() {
            return Ok(None);
        }

        if resolved.is_instance_of::<Uint8Array>() {
            let arr = Uint8Array::unchecked_from_js(resolved);
            return Ok(Some(crate::js_bytes::to_vec(&arr)));
        }

        Ok(None)
    }

    async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: &[u8],
        ttl: Option<Duration>,
    ) -> anyhow::Result<()> {
        let uint8 = Uint8Array::from(value);
        let ttl_js = match ttl {
            Some(d) => JsValue::from_f64(d.as_secs() as f64),
            None => JsValue::undefined(),
        };

        let result = self
            .set_fn
            .bind2(
                &JsValue::NULL,
                &JsValue::from_str(namespace),
                &JsValue::from_str(key),
            )
            .call2(&JsValue::NULL, &uint8.into(), &ttl_js)
            .map_err(|e| anyhow::anyhow!("cache set error: {e:?}"))?;

        resolve_promise(result).await?;
        Ok(())
    }

    async fn delete(&self, namespace: &str, key: &str) -> anyhow::Result<()> {
        let result = self
            .delete_fn
            .call2(
                &JsValue::NULL,
                &JsValue::from_str(namespace),
                &JsValue::from_str(key),
            )
            .map_err(|e| anyhow::anyhow!("cache delete error: {e:?}"))?;

        resolve_promise(result).await?;
        Ok(())
    }

    async fn clear(&self, namespace: &str) -> anyhow::Result<()> {
        let result = self
            .clear_fn
            .call1(&JsValue::NULL, &JsValue::from_str(namespace))
            .map_err(|e| anyhow::anyhow!("cache clear error: {e:?}"))?;

        resolve_promise(result).await?;
        Ok(())
    }
}

#[cfg(test)]
mod generated_host_callback_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    #[test]
    fn registration_keeps_getter_order_and_required_throw_identity() {
        let value=js_sys::eval(r#"(() => { const reads=[], obj={reads};
            for(const name of ["get", "set", "delete", "clear"]) Object.defineProperty(obj,name,{get(){reads.push(name);return ()=>{};}});
            return obj; })()"#).unwrap();
        JsCacheStoreAdapter::from_js(&value).unwrap();
        let reads: Vec<String> =
            serde_wasm_bindgen::from_value(js_sys::Reflect::get(&value, &"reads".into()).unwrap())
                .unwrap();
        assert_eq!(reads, vec!["get", "set", "delete", "clear"]);
        let value = js_sys::eval(
            r#"(() => {const marker={marker:true}; return {marker,get get(){throw marker;}};})()"#,
        )
        .unwrap();
        let marker = js_sys::Reflect::get(&value, &"marker".into()).unwrap();
        let error = JsCacheStoreAdapter::from_js(&value).err().unwrap();
        assert_eq!(error, marker);
        let value = js_sys::eval(r#"({get:42})"#).unwrap();
        let error = JsCacheStoreAdapter::from_js(&value).err().unwrap();
        assert_eq!(
            error.as_string().unwrap(),
            "cacheStore.get must be a function"
        );
    }
}
