//! `WhatsAppClientCore` API methods — engine-agnostic ports of the old
//! `wasm_client.rs` `WasmWhatsAppClient` methods. Logic lives here (the napi
//! binding only forwards primitives + materializes `BridgeValue`), so a future
//! engine swap re-implements only the thin binding, not these.
//!
//! Conversion contract: structured results are serialized to [`BridgeValue`]
//! (camelCase result types via `to_bridge_snake`, which respects their serde
//! `rename_all`); bytes/strings/scalars are returned directly. Errors are
//! [`BridgeError`].

use crate::client::WhatsAppClientCore;
use crate::errors::{self, BridgeError};
use crate::value::BridgeValue;
use crate::{helpers, result_types, serializer};

/// Serialize a result preserving serde key casing (result types already
/// `rename_all = "camelCase"`). Maps serializer errors to `BridgeError::Internal`.
pub(crate) fn snake<T: serde::Serialize>(v: &T) -> Result<BridgeValue, BridgeError> {
    serializer::to_bridge_snake(v).map_err(|e| errors::internal(e.to_string()))
}

/// camelCase serialization (for proto payloads whose serde field names are
/// snake_case). Maps serializer errors to `BridgeError::Internal`.
pub(crate) fn camel<T: serde::Serialize>(v: &T) -> Result<BridgeValue, BridgeError> {
    serializer::to_bridge_camel(v).map_err(|e| errors::internal(e.to_string()))
}

impl WhatsAppClientCore {
    // ── Messaging (bytes-based) ──────────────────────────────────────────────

    pub async fn send_message_bytes(&self, jid: &str, bytes: &[u8]) -> Result<String, BridgeError> {
        let (to, msg) = helpers::parse_jid_and_msg_bytes(jid, bytes)?;
        let result = self.client.send_message(to, msg).await?;
        Ok(result.message_id)
    }

    // ── Groups ───────────────────────────────────────────────────────────────

    pub async fn group_metadata(&self, jid: &str) -> Result<BridgeValue, BridgeError> {
        let group_jid = helpers::parse_jid(jid)?;
        let metadata = self.client.groups().get_metadata(&group_jid).await?;
        snake(&result_types::group_metadata_to_result(&metadata))
    }

    // ── MEX (GraphQL) ──────────────────────────────────────────────────────────

    pub async fn mex_query(
        &self,
        op_name: String,
        doc_id: String,
        variables_json: &str,
    ) -> Result<BridgeValue, BridgeError> {
        use wacore::iq::mex::MexDoc;
        use whatsapp_rust::features::MexRequest;

        let variables: serde_json::Value = serde_json::from_str(variables_json)
            .map_err(|e| errors::invalid_arg("variables", e.to_string()))?;
        let doc = MexDoc {
            name: helpers::intern_static(op_name),
            id: helpers::intern_static(doc_id),
        };
        let response = self
            .client
            .mex()
            .query(MexRequest { doc, variables })
            .await?;
        snake(&response)
    }
}
