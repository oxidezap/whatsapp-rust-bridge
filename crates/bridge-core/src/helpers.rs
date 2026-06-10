//! Engine-agnostic free helper functions used by the client methods.
//!
//! Ported 1:1 from the old `src/wasm_client.rs` (minus the `js_sys`/`JsValue`
//! layer): JID parsing, message-byte decoding, the `MexDoc` string interner,
//! base64-url encoding, and the media auth-error check. Error returns reference
//! [`crate::errors::BridgeError`].

use wacore_binary::jid::Jid;

/// Parse a JID string, mapping a parse failure to a structured `BridgeError`.
pub fn parse_jid(jid: &str) -> Result<Jid, crate::errors::BridgeError> {
    jid.parse().map_err(crate::errors::BridgeError::from)
}

/// Parse a `(jid, raw protobuf message)` pair into a typed `(Jid, Message)`.
/// The bytes are a serialized `waproto::whatsapp::Message`.
pub fn parse_jid_and_msg_bytes(
    jid: &str,
    bytes: &[u8],
) -> Result<(Jid, waproto::whatsapp::Message), crate::errors::BridgeError> {
    use prost::Message;
    let to = parse_jid(jid)?;
    let msg = waproto::whatsapp::Message::decode(bytes)
        .map_err(|e| crate::errors::internal(format!("invalid message bytes: {e}")))?;
    Ok((to, msg))
}

/// Intern a runtime `String` into a `&'static str`, deduplicating across
/// calls. Required because `wacore::iq::mex::MexDoc` carries `&'static str`
/// fields, but host-supplied strings are heap-allocated. The internal map
/// keeps total leaked memory bounded by the count of distinct doc names+ids
/// ever passed in (typically <50 per app lifetime).
pub fn intern_static(s: String) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MAP: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let map = MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&v) = g.get(&s) {
        return v;
    }
    let leaked: &'static str = Box::leak(s.clone().into_boxed_str());
    g.insert(s, leaked);
    leaked
}

/// Base64-URL-safe (no padding) encoding for upload tokens.
pub fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Check if an HTTP status code is a media auth error (401/403).
pub fn is_auth_error(status: u16) -> bool {
    matches!(status, 401 | 403)
}
