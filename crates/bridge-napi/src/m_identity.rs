//! Thin napi forwards for the "identity" batch. Logic lives in
//! `bridge_core::m_identity`; this layer only converts at the JS boundary.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::WasmWhatsAppClient;
use crate::methods::napi_err;
use crate::value_js::JsBridgeValue;

#[napi]
impl WasmWhatsAppClient {
    #[napi(js_name = "getPushName")]
    pub async fn get_push_name(&self) -> String {
        self.core.get_push_name().await
    }

    #[napi(js_name = "getJid")]
    pub async fn get_jid(&self) -> Option<String> {
        self.core.get_jid().await
    }

    #[napi(js_name = "getLid")]
    pub async fn get_lid(&self) -> Option<String> {
        self.core.get_lid().await
    }

    #[napi(js_name = "getAccount")]
    pub async fn get_account(&self) -> Result<JsBridgeValue> {
        self.core
            .get_account()
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "getMemoryDiagnostics")]
    pub async fn get_memory_diagnostics(&self) -> Result<JsBridgeValue> {
        self.core
            .get_memory_diagnostics()
            .await
            .map(JsBridgeValue)
            .map_err(napi_err)
    }

    #[napi(js_name = "setPushName")]
    pub async fn set_push_name(&self, name: String) -> Result<()> {
        self.core.set_push_name(&name).await.map_err(napi_err)
    }

    #[napi(js_name = "lidForPn")]
    pub async fn lid_for_pn(&self, jid: String) -> Result<Option<String>> {
        self.core.lid_for_pn(&jid).await.map_err(napi_err)
    }

    #[napi(js_name = "pnForLid")]
    pub async fn pn_for_lid(&self, jid: String) -> Result<Option<String>> {
        self.core.pn_for_lid(&jid).await.map_err(napi_err)
    }

    #[napi(js_name = "jidToSignalProtocolAddress")]
    pub fn jid_to_signal_protocol_address(&self, jid: String) -> Result<String> {
        self.core
            .jid_to_signal_protocol_address(&jid)
            .map_err(napi_err)
    }

    #[napi(js_name = "requestPairingCode")]
    pub async fn request_pairing_code(
        &self,
        phone_number: String,
        custom_code: Option<String>,
    ) -> Result<String> {
        self.core
            .request_pairing_code(&phone_number, custom_code)
            .await
            .map_err(napi_err)
    }

    #[napi(js_name = "setRawNodeForwarding")]
    pub fn set_raw_node_forwarding(&self, enabled: bool) {
        self.core.set_raw_node_forwarding(enabled);
    }

    #[napi(js_name = "sendNode")]
    pub async fn send_node(&self, node_js: serde_json::Value) -> Result<()> {
        self.core.send_node(node_js).await.map_err(napi_err)
    }

    #[napi(js_name = "sendRawMessage")]
    pub async fn send_raw_message(&self, data: Uint8Array) -> Result<()> {
        self.core.send_raw_message(&data).await.map_err(napi_err)
    }
}
