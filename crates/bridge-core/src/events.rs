//! wacore `Event` → [`BridgeValue`] serialization.
//!
//! Engine-agnostic port of the old `wasm_client.rs` `bridge_events!` macro
//! dispatch (`event_to_js`) plus `event_to_js_special`, dropping the `js_sys`/
//! `JsValue` layer. Each handled `Event` variant produces a
//! `{ type: "<name>", data: <BridgeValue> }` object; unhandled variants return
//! `None` (mirrors the old UNDEFINED-drop, which the dispatch loop skipped).
//!
//! Casing follows the old WASM split exactly:
//!   * the `serialize {}` variants (snake_case wacore types) go through
//!     [`serializer::to_bridge_snake`] (the old `proto::to_js_value` path);
//!   * `message` + `history_sync` proto payloads go through
//!     [`serializer::to_bridge_camel`] (protobufjs/Long path), with the injected
//!     metadata fields (`is_view_once`, `syncType`, `chunkOrder`, …) appended in
//!     their original camelCase / snake_case spelling;
//!   * the remaining special variants build the object by hand, mirroring the
//!     old `Reflect::set` shapes (node bytes → [`BridgeValue::Bytes`], children →
//!     [`BridgeValue::Array`]).
//!
//! HistorySync batching (the old `history_sync_to_js_batches`) is intentionally
//! dropped here: this entry point returns a single value, so we emit the one
//! `history_sync` event (the old `event_to_js_special` non-batched arm).

use wacore::types::events::{Event, LazyHistorySync};
use wacore_binary::node::{NodeContentRef, NodeRef, ValueRef};

use crate::serializer;
use crate::value::BridgeValue;

/// Serialize an `Event` into a `{ type, data }` [`BridgeValue::Object`], or
/// `None` for variants the bridge does not surface.
pub fn event_to_bridge(event: &Event) -> Option<BridgeValue> {
    // 1) `serialize {}` variants — snake_case via `to_bridge_snake`. Mirrors the
    //    old `event_to_js` dispatch arm (`crate::proto::to_js_value(data)`).
    match event {
        Event::Receipt(d) => snake("receipt", d),
        Event::UndecryptableMessage(d) => snake("undecryptable_message", d),
        Event::ChatPresence(d) => snake("chat_presence", d),
        Event::Presence(d) => snake("presence", d),
        Event::PictureUpdate(d) => snake("picture_update", d),
        Event::UserAboutUpdate(d) => snake("user_about_update", d),
        Event::ContactUpdated(d) => snake("contact_updated", d),
        Event::ContactNumberChanged(d) => snake("contact_number_changed", d),
        Event::ContactSyncRequested(d) => snake("contact_sync_requested", d),
        Event::GroupUpdate(d) => snake("group_update", d),
        Event::ContactUpdate(d) => snake("contact_update", d),
        Event::PushNameUpdate(d) => snake("push_name_update", d),
        Event::SelfPushNameUpdated(d) => snake("self_push_name_updated", d),
        Event::PinUpdate(d) => snake("pin_update", d),
        Event::MuteUpdate(d) => snake("mute_update", d),
        Event::ArchiveUpdate(d) => snake("archive_update", d),
        Event::StarUpdate(d) => snake("star_update", d),
        Event::MarkChatAsReadUpdate(d) => snake("mark_chat_as_read_update", d),
        Event::DeleteChatUpdate(d) => snake("delete_chat_update", d),
        Event::DeleteMessageForMeUpdate(d) => snake("delete_message_for_me_update", d),
        Event::OfflineSyncPreview(d) => snake("offline_sync_preview", d),
        Event::OfflineSyncCompleted(d) => snake("offline_sync_completed", d),
        Event::DeviceListUpdate(d) => snake("device_list_update", d),
        Event::IdentityChange(d) => snake("identity_change", d),
        Event::BusinessStatusUpdate(d) => snake("business_status_update", d),
        Event::TemporaryBan(d) => snake("temporary_ban", d),
        Event::ConnectFailure(d) => snake("connect_failure", d),
        Event::StreamError(d) => snake("stream_error", d),
        Event::DisappearingModeChanged(d) => snake("disappearing_mode_changed", d),
        Event::NewsletterLiveUpdate(d) => snake("newsletter_live_update", d),
        Event::IncomingCall(d) => snake("incoming_call", d),
        Event::MexNotification(d) => snake("mex_notification", d),
        // 2) special variants + unhandled — fall through to `event_to_special`.
        _ => event_to_special(event),
    }
}

/// Build a `{ type, data }` object whose `data` is the snake-case serialization
/// of `payload`. `None` on a serializer error (matches the old `?`-drop, which
/// logged and dropped the event).
fn snake<T: serde::Serialize>(event_type: &str, payload: &T) -> Option<BridgeValue> {
    let data = serializer::to_bridge_snake(payload).ok()?;
    Some(envelope(event_type, data))
}

/// Wrap `data` in `{ type, data }`. Key order matches the old
/// `Reflect::set("type")` then `Reflect::set("data")`.
fn envelope(event_type: &str, data: BridgeValue) -> BridgeValue {
    BridgeValue::Object(vec![
        ("type".to_string(), BridgeValue::str(event_type)),
        ("data".to_string(), data),
    ])
}

/// Host-agnostic port of `event_to_js_special`: variants that need a no-data,
/// named-field, multi-field, or proto-camel payload. Returns `None` for the
/// unhandled tail (the old UNDEFINED-drop).
fn event_to_special(event: &Event) -> Option<BridgeValue> {
    let empty = || BridgeValue::Object(Vec::new());

    let (event_type, data): (&str, BridgeValue) = match event {
        Event::Connected(_) => ("connected", empty()),
        Event::Disconnected(_) => ("disconnected", empty()),
        Event::QrScannedWithoutMultidevice(_) => ("qr_scanned_without_multidevice", empty()),
        Event::ClientOutdated(_) => ("client_outdated", empty()),
        Event::StreamReplaced(_) => ("stream_replaced", empty()),
        Event::PairingQrCode { code, timeout } => (
            "qr",
            BridgeValue::Object(vec![
                ("code".to_string(), BridgeValue::str(code.as_str())),
                (
                    "timeout".to_string(),
                    BridgeValue::F64(timeout.as_secs() as f64),
                ),
            ]),
        ),
        Event::PairingCode { code, timeout } => (
            "pairing_code",
            BridgeValue::Object(vec![
                ("code".to_string(), BridgeValue::str(code.as_str())),
                (
                    "timeout".to_string(),
                    BridgeValue::F64(timeout.as_secs() as f64),
                ),
            ]),
        ),
        Event::PairSuccess(ps) => (
            "pair_success",
            BridgeValue::Object(vec![
                ("id".to_string(), BridgeValue::str(ps.id.to_string())),
                ("lid".to_string(), BridgeValue::str(ps.lid.to_string())),
                (
                    "business_name".to_string(),
                    BridgeValue::str(ps.business_name.as_str()),
                ),
                (
                    "platform".to_string(),
                    BridgeValue::str(ps.platform.as_str()),
                ),
            ]),
        ),
        Event::PairError(pe) => (
            "pair_error",
            BridgeValue::Object(vec![
                ("id".to_string(), BridgeValue::str(pe.id.to_string())),
                ("lid".to_string(), BridgeValue::str(pe.lid.to_string())),
                (
                    "business_name".to_string(),
                    BridgeValue::str(pe.business_name.as_str()),
                ),
                (
                    "platform".to_string(),
                    BridgeValue::str(pe.platform.as_str()),
                ),
                ("error".to_string(), BridgeValue::str(pe.error.as_str())),
            ]),
        ),
        Event::LoggedOut(lo) => (
            "logged_out",
            BridgeValue::Object(vec![
                ("on_connect".to_string(), BridgeValue::Bool(lo.on_connect)),
                (
                    "reason".to_string(),
                    BridgeValue::str(format!("{:?}", lo.reason)),
                ),
            ]),
        ),
        Event::Message(msg, info) => {
            use wacore::proto_helpers::MessageExt;
            // Proto message → camelCase (matches protobufjs/Baileys convention).
            let message = serializer::to_bridge_camel(msg.as_ref()).ok()?;
            // MessageInfo → snake_case (wacore type), with `is_view_once` injected.
            let mut info_obj = match serializer::to_bridge_snake(info.as_ref()).ok()? {
                BridgeValue::Object(fields) => fields,
                other => return Some(envelope("message", other)),
            };
            info_obj.push((
                "is_view_once".to_string(),
                BridgeValue::Bool(msg.is_view_once()),
            ));
            (
                "message",
                BridgeValue::Object(vec![
                    ("message".to_string(), message),
                    ("info".to_string(), BridgeValue::Object(info_obj)),
                ]),
            )
        }
        Event::Notification(node) => ("notification", node_ref_to_bridge(node.get())),
        Event::RawNode(node) => ("raw_node", node_ref_to_bridge(node.get())),
        Event::HistorySync(lazy) => {
            // Drop if the proto blob can't be decoded — consumers can't act on it.
            let parsed = lazy.get()?;
            let data = serializer::to_bridge_camel(parsed).ok()?;
            ("history_sync", overlay_history_sync_meta(data, lazy))
        }
        // All other variants are unhandled (the old UNDEFINED-drop).
        _ => return None,
    };

    Some(envelope(event_type, data))
}

/// Overlay the notification-level metadata onto a serialized HistorySync,
/// mirroring the old `event_to_js_special` HistorySync arm. `syncType` is
/// always present; `chunkOrder`/`progress`/`peerDataRequestSessionId` only when
/// the source carries them (`peerDataRequestSessionId` is ON_DEMAND-only).
fn overlay_history_sync_meta(data: BridgeValue, lazy: &LazyHistorySync) -> BridgeValue {
    let mut fields = match data {
        BridgeValue::Object(f) => f,
        other => return other,
    };
    fields.push((
        "syncType".to_string(),
        BridgeValue::F64(lazy.sync_type() as f64),
    ));
    if let Some(co) = lazy.chunk_order() {
        fields.push(("chunkOrder".to_string(), BridgeValue::F64(co as f64)));
    }
    if let Some(p) = lazy.progress() {
        fields.push(("progress".to_string(), BridgeValue::F64(p as f64)));
    }
    if let Some(sid) = lazy.peer_data_request_session_id() {
        fields.push((
            "peerDataRequestSessionId".to_string(),
            BridgeValue::str(sid),
        ));
    }
    BridgeValue::Object(fields)
}

/// Convert a yoke-borrowed `NodeRef` → a `{ tag, attrs, content }`
/// [`BridgeValue::Object`]. Port of the old `node_ref_to_js`, zero-copy over the
/// decoded buffer. `attrs` is an object of `string` values; `content` is a child
/// array, a string, or raw bytes (omitted when absent).
fn node_ref_to_bridge(node: &NodeRef<'_>) -> BridgeValue {
    let mut obj: Vec<(String, BridgeValue)> = Vec::with_capacity(3);
    obj.push(("tag".to_string(), BridgeValue::str(&*node.tag)));

    let attrs: Vec<(String, BridgeValue)> = node
        .attrs_iter()
        .map(|(key, value)| {
            let val = match value {
                ValueRef::String(s) => BridgeValue::str(&**s),
                ValueRef::Jid(j) => {
                    let mut s = String::with_capacity(j.user.len() + 20);
                    wacore_binary::push_jid_to_string(&j.user, j.server, j.agent, j.device, &mut s);
                    BridgeValue::Str(s)
                }
            };
            (key.to_string(), val)
        })
        .collect();
    obj.push(("attrs".to_string(), BridgeValue::Object(attrs)));

    if let Some(content) = node.content.as_deref() {
        let content_val = match content {
            NodeContentRef::Nodes(children) => {
                BridgeValue::Array(children.iter().map(node_ref_to_bridge).collect())
            }
            NodeContentRef::Bytes(bytes) => BridgeValue::Bytes(bytes.as_ref().to_vec()),
            NodeContentRef::String(s) => BridgeValue::str(&**s),
        };
        obj.push(("content".to_string(), content_val));
    }

    BridgeValue::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use std::time::Duration;
    use wacore::types::events::{Connected, PairSuccess};
    use wacore_binary::node::{Attrs, Node, NodeContent, NodeValue};

    /// Pull a field out of an `Object`, panicking on shape mismatch.
    fn field<'a>(v: &'a BridgeValue, key: &str) -> &'a BridgeValue {
        let BridgeValue::Object(fields) = v else {
            panic!("expected object, got {v:?}")
        };
        &fields
            .iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("missing key {key} in {fields:?}"))
            .1
    }

    #[test]
    fn serialize_variant_wraps_in_typed_envelope() {
        let ev =
            Event::OfflineSyncCompleted(wacore::types::events::OfflineSyncCompleted { count: 7 });
        let out = event_to_bridge(&ev).expect("handled");
        assert_eq!(
            field(&out, "type"),
            &BridgeValue::str("offline_sync_completed")
        );
        // snake mode keeps the field name verbatim and emits the small int as F64.
        assert_eq!(field(field(&out, "data"), "count"), &BridgeValue::F64(7.0));
    }

    #[test]
    fn no_data_special_variant_is_empty_object() {
        let out = event_to_bridge(&Event::Connected(Connected)).expect("handled");
        assert_eq!(field(&out, "type"), &BridgeValue::str("connected"));
        assert_eq!(field(&out, "data"), &BridgeValue::Object(Vec::new()));
    }

    #[test]
    fn qr_timeout_is_seconds_as_number() {
        let ev = Event::PairingQrCode {
            code: "ABCD".to_string(),
            timeout: Duration::from_secs(20),
        };
        let out = event_to_bridge(&ev).expect("handled");
        let data = field(&out, "data");
        assert_eq!(field(data, "code"), &BridgeValue::str("ABCD"));
        assert_eq!(field(data, "timeout"), &BridgeValue::F64(20.0));
    }

    #[test]
    fn pair_success_stringifies_jids() {
        let ev = Event::PairSuccess(PairSuccess {
            id: "123@s.whatsapp.net".parse().unwrap(),
            lid: "456@lid".parse().unwrap(),
            business_name: "Acme".to_string(),
            platform: "android".to_string(),
        });
        let out = event_to_bridge(&ev).expect("handled");
        let data = field(&out, "data");
        assert_eq!(field(data, "business_name"), &BridgeValue::str("Acme"));
        assert!(matches!(field(data, "id"), BridgeValue::Str(_)));
    }

    #[test]
    fn node_ref_to_bridge_maps_tag_attrs_and_content() {
        let mut attrs = Attrs::new();
        attrs.push(
            Cow::Borrowed("from"),
            NodeValue::String("u@s.whatsapp.net".into()),
        );
        let child = Node::new(
            Cow::Borrowed("child"),
            Attrs::new(),
            Some(NodeContent::Bytes(vec![1, 2, 3])),
        );
        let node = Node::new(
            Cow::Borrowed("message"),
            attrs,
            Some(NodeContent::Nodes(vec![child])),
        );

        let out = node_ref_to_bridge(&node.as_node_ref());
        assert_eq!(field(&out, "tag"), &BridgeValue::str("message"));
        assert_eq!(
            field(field(&out, "attrs"), "from"),
            &BridgeValue::str("u@s.whatsapp.net")
        );
        let BridgeValue::Array(children) = field(&out, "content") else {
            panic!("content should be an array of child nodes")
        };
        assert_eq!(children.len(), 1);
        assert_eq!(field(&children[0], "tag"), &BridgeValue::str("child"));
        assert_eq!(
            field(&children[0], "content"),
            &BridgeValue::Bytes(vec![1, 2, 3])
        );
    }
}
