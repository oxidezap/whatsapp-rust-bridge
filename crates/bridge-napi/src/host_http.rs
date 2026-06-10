//! `HostHttp` over a JS `execute(url, method, headers, body?) => Promise<{statusCode, body}>`
//! ThreadsafeFunction. Built (CalleeHandled=false) from the config object's
//! `execute` method in `createWhatsAppClient`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;
use wacore::net::{HttpRequest, HttpResponse};

use bridge_core::host::HostHttp;

/// JS return shape `{ statusCode: number; body: Uint8Array }` (napi camelCases).
#[napi(object)]
pub struct HttpExecResult {
    pub status_code: u32,
    pub body: Uint8Array,
}

pub type ExecArgsJs = FnArgs<(String, String, HashMap<String, String>, Option<Uint8Array>)>;
pub type ExecuteTsfn =
    ThreadsafeFunction<ExecArgsJs, Promise<HttpExecResult>, ExecArgsJs, Status, false>;

pub struct NapiHttp {
    pub execute: Arc<ExecuteTsfn>,
}

#[async_trait]
impl HostHttp for NapiHttp {
    async fn execute(&self, req: HttpRequest) -> anyhow::Result<HttpResponse> {
        let body = req.body.map(|b| Uint8Array::from(b.to_vec()));
        let args = FnArgs::from((req.url, req.method, req.headers, body));
        let promise = self
            .execute
            .call_async(args)
            .await
            .map_err(|e| anyhow::anyhow!("http execute dispatch: {e}"))?;
        let res = promise
            .await
            .map_err(|e| anyhow::anyhow!("http execute promise: {e}"))?;
        Ok(HttpResponse {
            status_code: res.status_code as u16,
            body: res.body.to_vec(),
        })
    }
}
