//! Identity / low-level methods ported 1:1 from `wasm_client.rs`:
//! `getJid`, `getLid`, `getPushName`, `setPushName`, `getAccount`, `lidForPn`,
//! `pnForLid`, `jidToSignalProtocolAddress` (sync), `requestPairingCode`,
//! `getMemoryDiagnostics`, `setRawNodeForwarding` (sync), `sendNode`,
//! `sendRawMessage`.
//!
//! The only thing that changes versus the old WASM bodies is the JS-interop
//! boundary: `sendNode` no longer takes a `JsValue` — it takes a
//! `serde_json::Value` (the napi binding deserializes the JS `BinaryNode`),
//! and `getAccount` returns a `BridgeValue` (materialized to JS by the
//! binding) instead of a `JsValue`.

use crate::client::WhatsAppClientCore;
use crate::errors::{self, BridgeError};
use crate::methods::camel;
use crate::value::BridgeValue;
use crate::{helpers, result_types};

use wacore_binary::jid::Jid;

impl WhatsAppClientCore {
    // ── State getters ────────────────────────────────────────────────────

    /// Get the current push name.
    pub async fn get_push_name(&self) -> String {
        self.client.get_push_name().await
    }

    /// Get the own JID (phone number JID) if logged in. Returns the non-AD JID
    /// (without device suffix), e.g. "559980000014@s.whatsapp.net".
    pub async fn get_jid(&self) -> Option<String> {
        self.client
            .get_pn()
            .await
            .map(|j| j.to_non_ad().to_string())
    }

    /// Get the own LID (linked identity) if available. Returns the non-AD LID
    /// (without device suffix), e.g. "100000012345678@lid".
    pub async fn get_lid(&self) -> Option<String> {
        self.client
            .get_lid()
            .await
            .map(|j| j.to_non_ad().to_string())
    }

    /// Get the ADV signed device identity (account), if available. Returns
    /// `BridgeValue::Null` when absent (the old WASM body returned `undefined`).
    pub async fn get_account(&self) -> Result<BridgeValue, BridgeError> {
        let snapshot = self.persistence_manager.get_device_snapshot().await;
        match &snapshot.account {
            Some(account) => camel(account),
            None => Ok(BridgeValue::Null),
        }
    }

    /// Returns a snapshot of internal memory diagnostics (cache sizes, session
    /// counts, etc.).
    pub async fn get_memory_diagnostics(&self) -> Result<BridgeValue, BridgeError> {
        let d = self.client.memory_diagnostics().await;
        let result = result_types::MemoryDiagnosticsResult {
            group_cache: d.group_cache as f64,
            device_registry_cache: d.device_registry_cache as f64,
            sender_key_device_cache: d.sender_key_device_cache as f64,
            lid_pn_lid_entries: d.lid_pn_lid_entries as f64,
            lid_pn_pn_entries: d.lid_pn_pn_entries as f64,
            recent_messages: d.recent_messages as f64,
            message_retry_counts: d.message_retry_counts as f64,
            pdo_pending_requests: d.pdo_pending_requests as f64,
            session_locks: d.session_locks as f64,
            chat_lanes: d.chat_lanes as f64,
            response_waiters: d.response_waiters as f64,
            node_waiters: d.node_waiters as f64,
            pending_retries: d.pending_retries as f64,
            presence_subscriptions: d.presence_subscriptions as f64,
            app_state_key_requests: d.app_state_key_requests as f64,
            app_state_syncing: d.app_state_syncing as f64,
            signal_cache_sessions: d.signal_cache_sessions as f64,
            signal_cache_identities: d.signal_cache_identities as f64,
            signal_cache_sender_keys: d.signal_cache_sender_keys as f64,
            chatstate_handlers: d.chatstate_handlers as f64,
            custom_enc_handlers: d.custom_enc_handlers as f64,
        };
        crate::methods::snake(&result)
    }

    // ── Profile ──────────────────────────────────────────────────────────

    /// Set the user's push name (display name).
    pub async fn set_push_name(&self, name: &str) -> Result<(), BridgeError> {
        self.client
            .profile()
            .set_push_name(name)
            .await
            .map_err(BridgeError::from)
    }

    // ── LID/PN mapping ────────────────────────────────────────────────────

    /// Look up the LID JID corresponding to a given phone-number JID. Accepts a
    /// bare phone number (treated as PN), a `<phone>@s.whatsapp.net` JID, or any
    /// LID/PN JID. Returns the full LID JID string or `None`.
    pub async fn lid_for_pn(&self, jid: &str) -> Result<Option<String>, BridgeError> {
        let parsed = if jid.contains('@') {
            helpers::parse_jid(jid)?
        } else {
            Jid::pn(jid)
        };
        Ok(self
            .client
            .get_lid_pn_entry(&parsed)
            .await?
            .map(|e| format!("{}@lid", e.lid)))
    }

    /// Look up the phone number JID corresponding to a given LID JID. Accepts a
    /// bare LID user-part, a `<user>@lid` JID, or any LID/PN JID. Returns the
    /// full PN JID string or `None`.
    pub async fn pn_for_lid(&self, jid: &str) -> Result<Option<String>, BridgeError> {
        let parsed = if jid.contains('@') {
            helpers::parse_jid(jid)?
        } else {
            Jid::lid(jid)
        };
        Ok(self
            .client
            .get_lid_pn_entry(&parsed)
            .await?
            .map(|e| format!("{}@s.whatsapp.net", e.phone_number)))
    }

    /// Convert a JID string to its Signal protocol address representation.
    pub fn jid_to_signal_protocol_address(&self, jid: &str) -> Result<String, BridgeError> {
        use wacore::types::jid::JidExt;
        let parsed = helpers::parse_jid(jid)?;
        Ok(parsed.to_protocol_address_string())
    }

    // ── Pairing ───────────────────────────────────────────────────────────

    /// Request a pairing code for phone number login (alternative to QR).
    /// Returns the 8-character pairing code to enter on the phone.
    pub async fn request_pairing_code(
        &self,
        phone_number: &str,
        custom_code: Option<String>,
    ) -> Result<String, BridgeError> {
        use whatsapp_rust::pair_code::PairCodeOptions;
        let options = PairCodeOptions {
            phone_number: phone_number.to_string(),
            custom_code,
            ..Default::default()
        };
        let code = self.client.pair_with_code(options).await?;
        Ok(code)
    }

    // ── Signal / low-level protocol ────────────────────────────────────────

    /// Enable or disable raw node forwarding. When enabled, a `raw_node` event
    /// is emitted for every decoded stanza before internal dispatch.
    pub fn set_raw_node_forwarding(&self, enabled: bool) {
        self.client.set_raw_node_forwarding(enabled);
    }

    /// Send a raw binary node stanza to WhatsApp servers. `node` is the
    /// deserialized JS `BinaryNode`: `{ tag, attrs: Record<string,string>,
    /// content?: string | bytes | BinaryNode[] }`.
    pub async fn send_node(&self, node: serde_json::Value) -> Result<(), BridgeError> {
        let node = json_to_node(&node)?;
        self.client.send_node(node).await.map_err(BridgeError::from)
    }

    // ── Raw transport ──────────────────────────────────────────────────────

    /// Send pre-marshaled bytes through the noise socket.
    pub async fn send_raw_message(&self, data: &[u8]) -> Result<(), BridgeError> {
        self.client
            .send_raw_bytes(data.to_vec())
            .await
            .map_err(BridgeError::from)
    }
}

/// Convert a deserialized JS `BinaryNode` (`serde_json::Value`) → wacore `Node`.
///
/// Ported from the old `js_to_node` (`wasm_client.rs`) — only the source value
/// type changed (`JsValue` → `serde_json::Value`). Semantics preserved:
/// * attr values containing `'@'` that parse as a `Jid` become `NodeValue::Jid`,
///   everything else `NodeValue::String`;
/// * string content → `NodeContent::String`; an array of `BinaryNode` objects →
///   `NodeContent::Nodes`; an array of numbers (a `Uint8Array` materialized by
///   napi's serde bridge) → `NodeContent::Bytes`.
fn json_to_node(val: &serde_json::Value) -> Result<wacore_binary::node::Node, BridgeError> {
    use std::borrow::Cow;
    use wacore_binary::node::{Attrs, Node, NodeContent, NodeValue};

    let tag = val
        .get("tag")
        .and_then(|t| t.as_str())
        .ok_or_else(|| errors::internal("tag must be a string"))?
        .to_string();

    // attrs: Record<string, string> → Vec<(Cow, NodeValue)>
    let mut attrs = Attrs::new();
    if let Some(map) = val.get("attrs").and_then(|a| a.as_object()) {
        for (key, value) in map {
            let val_str = value.as_str().unwrap_or_default().to_string();
            if val_str.contains('@')
                && let Ok(jid) = val_str.parse::<wacore_binary::jid::Jid>()
            {
                attrs.push(Cow::Owned(key.clone()), NodeValue::Jid(jid));
                continue;
            }
            attrs.push(Cow::Owned(key.clone()), NodeValue::String(val_str.into()));
        }
    }

    // content: string | Uint8Array (number[]) | BinaryNode[]
    let content = match val.get("content") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(NodeContent::String(s.as_str().into())),
        Some(serde_json::Value::Array(arr)) => {
            // A Uint8Array materializes as an array of numbers; a children list
            // materializes as an array of objects. Disambiguate on the first
            // element (an empty array carries no bytes → empty children list).
            if arr.first().map(|v| v.is_number()).unwrap_or(false) {
                let bytes = arr.iter().map(|v| v.as_u64().unwrap_or(0) as u8).collect();
                Some(NodeContent::Bytes(bytes))
            } else {
                let mut children = Vec::with_capacity(arr.len());
                for child in arr {
                    children.push(json_to_node(child)?);
                }
                Some(NodeContent::Nodes(children))
            }
        }
        Some(_) => None,
    };

    Ok(Node::new(Cow::Owned(tag), attrs, content))
}
