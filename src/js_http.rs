//! JS fetch()-based HTTP client adapter.
//!
//! Uses raw `js_sys::Function` to avoid wasm-bindgen reentrancy issues.

use async_trait::async_trait;
use js_sys::{Object, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use whatsapp_rust::wacore::net::{HttpClient, HttpRequest, HttpResponse};

generated_http_callbacks!();

crate::wasm_send_sync!(JsHttpClientAdapter);

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl HttpClient for JsHttpClientAdapter {
    async fn execute(&self, request: HttpRequest) -> anyhow::Result<HttpResponse> {
        let headers_obj = Object::new();
        for (key, value) in &request.headers {
            js_sys::Reflect::set(&headers_obj, &key.into(), &value.into())
                .map_err(|e| anyhow::anyhow!("set header: {e:?}"))?;
        }

        let body_js = match &request.body {
            // #774: HttpRequest.body is now `Bytes` (was Vec<u8>); index to a &[u8] slice.
            Some(b) => Uint8Array::from(&b[..]).into(),
            None => JsValue::NULL,
        };

        let result = self
            .execute_fn
            .call4(
                &JsValue::NULL,
                &request.url.into(),
                &request.method.into(),
                &headers_obj.into(),
                &body_js,
            )
            .map_err(|e| anyhow::anyhow!("http execute: {e:?}"))?;

        let resolved = if result.is_instance_of::<js_sys::Promise>() {
            JsFuture::from(js_sys::Promise::unchecked_from_js(result))
                .await
                .map_err(|e| anyhow::anyhow!("http await: {e:?}"))?
        } else {
            result
        };

        let status_code = js_sys::Reflect::get(&resolved, &"statusCode".into())
            .map_err(|e| anyhow::anyhow!("get statusCode: {e:?}"))?
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("statusCode not a number"))?
            as u16;

        let body_val = js_sys::Reflect::get(&resolved, &"body".into())
            .map_err(|e| anyhow::anyhow!("get body: {e:?}"))?;

        let body = if body_val.is_instance_of::<Uint8Array>() {
            crate::js_bytes::to_vec(&Uint8Array::unchecked_from_js(body_val))
        } else {
            Vec::new()
        };

        Ok(HttpResponse { status_code, body })
    }
}

#[cfg(test)]
mod generated_host_callback_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    #[test]
    fn registration_keeps_getter_order_and_required_throw_identity() {
        let value=js_sys::eval(r#"(() => { const reads=[], obj={reads};
            for(const name of ["execute"]) Object.defineProperty(obj,name,{get(){reads.push(name);return ()=>{};}});
            return obj; })()"#).unwrap();
        JsHttpClientAdapter::from_js(value.clone()).unwrap();
        let reads: Vec<String> =
            serde_wasm_bindgen::from_value(js_sys::Reflect::get(&value, &"reads".into()).unwrap())
                .unwrap();
        assert_eq!(reads, vec!["execute"]);
        let value=js_sys::eval(r#"(() => {const marker={marker:true}; return {marker,get execute(){throw marker;}};})()"#).unwrap();
        let marker = js_sys::Reflect::get(&value, &"marker".into()).unwrap();
        let error = JsHttpClientAdapter::from_js(value.clone()).err().unwrap();
        assert_eq!(error, marker);
        let value = js_sys::eval(r#"({execute:42})"#).unwrap();
        let error = JsHttpClientAdapter::from_js(value.clone()).err().unwrap();
        assert_eq!(
            error.as_string().unwrap(),
            "httpClient.execute must be a function"
        );
    }
}
