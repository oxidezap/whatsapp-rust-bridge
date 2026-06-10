//! Thin napi forwards to `WhatsAppClientCore` methods. The binding only converts
//! at the boundary (`Uint8Array` ↔ bytes, `BridgeValue` → JS via `JsBridgeValue`,
//! `BridgeError` → napi exception); all logic lives in `bridge-core`.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::WasmWhatsAppClient;
use crate::value_js::JsBridgeValue;

/// Map a `bridge-core` error into a napi exception. TODO(parity): structured
/// `WhatsAppError` (`.kind` + fields); message preserved for now.
pub(crate) fn napi_err(e: impl std::fmt::Display) -> Error {
    Error::from_reason(e.to_string())
}

#[napi]
impl WasmWhatsAppClient {
    #[napi(js_name = "sendMessageBytes")]
    pub async fn send_message_bytes(&self, jid: String, bytes: Uint8Array) -> Result<String> {
        self.core
            .send_message_bytes(&jid, &bytes)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "getGroupMetadata")]
    pub async fn group_metadata(&self, jid: String) -> Result<JsBridgeValue> {
        self.core
            .group_metadata(&jid)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "mexQuery")]
    pub async fn mex_query(
        &self,
        op_name: String,
        doc_id: String,
        variables_json: String,
    ) -> Result<JsBridgeValue> {
        self.core
            .mex_query(op_name, doc_id, &variables_json)
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }
}
