//! Packed wire-batch transport: fixed-width numeric records plus a per-batch
//! deduplicated string table, so repeated addresses and metadata pay one
//! host-side decode per batch instead of one FFI object build per event.
//! Record layouts are mirrored by the host decoders in `ts/wire-info.ts`.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::js_keys;
use wasm_bindgen::JsValue;
use whatsapp_rust::wacore;
use whatsapp_rust::wacore::types::events::Event;
use whatsapp_rust::wacore_binary::jid::Jid;
use whatsapp_rust::waproto;

/// One packed run of same-kind events, coalesced by the dispatch loop in
/// `wasm_client`. The JS delivery channel stays with the callbacks owner.
pub(crate) trait PackedEventBatch: Default {
    /// Label for dispatch failure logs.
    const KIND: &'static str;
    /// Whether `event` belongs to this batch kind.
    fn accepts(event: &Event) -> bool;
    /// Run `f` with this kind's process-wide encoder. The encoder is reused
    /// so its string/JID caches persist across batches, which is what makes a
    /// batch of one worth packing.
    fn with_encoder<R>(f: impl FnOnce(&mut Self) -> R) -> R;
    /// Called once before a batch is filled, so the encoder can roll its
    /// persistent caches when they reach their ceiling.
    fn begin(&mut self);
    fn push(&mut self, event: &Event) -> Result<(), JsValue>;
    /// Assemble the batch for the host and reset the per-batch state; the
    /// caches survive so the next batch keeps interning against them.
    fn finish(&mut self) -> Result<JsValue, JsValue>;
}

/// Slots per packed metadata record. Layout is mirrored by the host-side
/// decoder (`ts/wire-info.ts`); update both together.
pub(crate) const MESSAGE_WIRE_INFO_RECORD_WIDTH: usize = 10;
const INFO_FLAG_FROM_ME: u32 = 1 << 0;
const INFO_FLAG_GROUP: u32 = 1 << 1;
const INFO_FLAG_VIEW_ONCE: u32 = 1 << 2;
const INFO_FLAG_OFFLINE: u32 = 1 << 3;

/// Per-batch deduplicated string table shared by every packed wire batch.
/// Repeated values (addresses, push names, ack classes) pay one host-side
/// decode per batch instead of one FFI string crossing per event.
#[derive(Default)]
pub(crate) struct WireStringTable {
    /// Concatenated UTF-8 payloads of the table entries.
    data: Vec<u8>,
    /// K + 1 byte offsets delimiting the K entries (leading zero).
    offsets: Vec<u32>,
    /// Build-time dedup index; never crosses the FFI.
    index: HashMap<String, u32>,
}

impl WireStringTable {
    fn intern(&mut self, value: &str) -> u32 {
        if let Some(&index) = self.index.get(value) {
            return index;
        }
        if self.offsets.is_empty() {
            self.offsets.push(0);
        }
        let index = (self.offsets.len() - 1) as u32;
        self.data.extend_from_slice(value.as_bytes());
        self.offsets.push(self.data.len() as u32);
        self.index.insert(value.to_owned(), index);
        index
    }

    /// Optional slots carry index + 1 so 0 can mean absent.
    #[inline]
    fn intern_optional(&mut self, value: Option<&str>) -> f64 {
        match value {
            Some(value) => (self.intern(value) + 1) as f64,
            None => 0.0,
        }
    }

    fn set_js(
        mut self,
        batch: &js_sys::Object,
        data_key: &'static std::thread::LocalKey<JsValue>,
        offsets_key: &'static std::thread::LocalKey<JsValue>,
    ) -> Result<(), JsValue> {
        js_keys::set(
            batch,
            data_key,
            &js_sys::Uint8Array::from(self.data.as_slice()).into(),
        )?;
        if self.offsets.is_empty() {
            self.offsets.push(0);
        }
        js_keys::set(
            batch,
            offsets_key,
            &js_sys::Uint32Array::from(self.offsets.as_slice()).into(),
        )?;
        Ok(())
    }
}

/// Bounded transport representation for decrypted messages. Protobuf payloads
/// share one backing buffer and one offset table. Metadata crosses as packed
/// numeric records plus a per-batch string table: addresses and push names
/// repeat across a batch, so each unique string pays one decode on the host
/// instead of one FFI object build per message.
#[derive(Default)]
pub(crate) struct MessageWireBatch {
    /// Concatenated `proto.Message` payloads in event order.
    message_data: Vec<u8>,
    /// N + 1 byte offsets delimiting the N payloads in `messageData`. The
    /// leading zero sentinel makes empty and singleton batches unambiguous.
    message_offsets: Vec<u32>,
    strings: WireStringTable,
    /// `MESSAGE_WIRE_INFO_RECORD_WIDTH` slots per message; string slots hold
    /// table indices (optional slots are index + 1, 0 = absent).
    info_records: Vec<f64>,
}

impl MessageWireBatch {
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.info_records.len() / MESSAGE_WIRE_INFO_RECORD_WIDTH
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.info_records.is_empty()
    }

    #[cfg_attr(
        feature = "memory-profiling",
        tracing::instrument(name = "bridge.event.message_wire.encode", level = "trace", skip_all)
    )]
    pub(crate) fn push(
        &mut self,
        inbound: &wacore::types::events::InboundMessage,
    ) -> Result<(), JsValue> {
        use wacore::proto_helpers::MessageExt;

        let previous_len = self.message_data.len();
        waproto::codec::message_encode_into(&inbound.message, &mut self.message_data);
        let end = match u32::try_from(self.message_data.len()) {
            Ok(end) => end,
            Err(_) => {
                self.message_data.truncate(previous_len);
                return Err(JsValue::from_str(
                    "message wire batch exceeded the Uint32 offset range",
                ));
            }
        };
        if self.message_offsets.is_empty() {
            self.message_offsets.push(0);
        }
        self.message_offsets.push(end);

        let info = inbound.info.as_ref();
        let source = &info.source;
        let edit = info.edit.to_string_val();
        let mut flags = 0u32;
        if source.is_from_me {
            flags |= INFO_FLAG_FROM_ME;
        }
        if source.is_group {
            flags |= INFO_FLAG_GROUP;
        }
        if inbound.message.is_view_once() {
            flags |= INFO_FLAG_VIEW_ONCE;
        }
        if info.is_offline {
            flags |= INFO_FLAG_OFFLINE;
        }

        let chat = self.strings.intern(&source.chat.to_non_ad_string()) as f64;
        let sender = self.strings.intern(&source.sender.to_non_ad_string()) as f64;
        let sender_alt = self.strings.intern_optional(
            source
                .sender_alt
                .as_ref()
                .map(Jid::to_non_ad_string)
                .as_deref(),
        );
        let recipient_alt = self.strings.intern_optional(
            source
                .recipient_alt
                .as_ref()
                .map(Jid::to_non_ad_string)
                .as_deref(),
        );
        let id = self.strings.intern(&info.id) as f64;
        let push_name = self.strings.intern(&info.push_name) as f64;
        let unavailable_request_id = self
            .strings
            .intern_optional(info.unavailable_request_id.as_deref());
        let edit = self
            .strings
            .intern_optional((!edit.is_empty()).then_some(edit));

        self.info_records.extend_from_slice(&[
            chat,
            sender,
            sender_alt,
            recipient_alt,
            id,
            push_name,
            info.timestamp.timestamp() as f64,
            flags as f64,
            unavailable_request_id,
            edit,
        ]);
        Ok(())
    }

    #[cfg_attr(
        feature = "memory-profiling",
        tracing::instrument(name = "bridge.event.message_wire.ffi", level = "trace", skip_all)
    )]
    pub(crate) fn into_js(self) -> Result<JsValue, JsValue> {
        let batch = js_sys::Object::new();
        js_keys::set(
            &batch,
            &js_keys::MESSAGE_WIRE_DATA_KEY,
            &js_sys::Uint8Array::from(self.message_data.as_slice()).into(),
        )?;
        let mut message_offsets = self.message_offsets;
        if message_offsets.is_empty() {
            message_offsets.push(0);
        }
        js_keys::set(
            &batch,
            &js_keys::MESSAGE_WIRE_OFFSETS_KEY,
            &js_sys::Uint32Array::from(message_offsets.as_slice()).into(),
        )?;
        self.strings.set_js(
            &batch,
            &js_keys::MESSAGE_WIRE_INFO_STRING_DATA_KEY,
            &js_keys::MESSAGE_WIRE_INFO_STRING_OFFSETS_KEY,
        )?;
        js_keys::set(
            &batch,
            &js_keys::MESSAGE_WIRE_INFO_RECORDS_KEY,
            &js_sys::Float64Array::from(self.info_records.as_slice()).into(),
        )?;
        Ok(batch.into())
    }
}

/// Cache ceilings before the encoder resets both sides. Bounded so a long
/// session cannot grow the tables without limit; a reset costs one batch of
/// re-definitions.
const PACKED_STRING_CACHE_MAX: usize = 4096;
const PACKED_JID_CACHE_MAX: usize = 1024;

/// Header flag: the host must clear its caches before reading this batch.
const PACKED_FLAG_RESET_CACHES: u32 = 1;

const RECEIPT_FLAG_FROM_ME: u8 = 1 << 0;
const RECEIPT_FLAG_GROUP: u8 = 1 << 1;
const RECEIPT_FLAG_OFFLINE: u8 = 1 << 2;
/// The type slot holds an `Other(inner)` payload instead of a variant name.
const RECEIPT_FLAG_TYPE_OTHER: u8 = 1 << 3;

/// Serde representation of `ReceiptType`: the packed record carries the
/// variant name, or the inner payload with the `Other` flag set. The core's
/// `variant_name` backs its own `Serialize`, so this stays in step with the
/// single-event shape without a mirrored list or a serde round-trip.
pub(crate) fn receipt_type_repr(value: &wacore::types::presence::ReceiptType) -> (&str, bool) {
    use wacore::types::presence::ReceiptType;
    match value {
        ReceiptType::Other(inner) => (inner.as_str(), true),
        known => (known.variant_name(), false),
    }
}

/// Flat batch writer shared by the packed event transports.
///
/// One contiguous buffer crosses per batch instead of several typed arrays,
/// and the string/JID caches PERSIST across batches: a repeated address or ack
/// class costs two bytes on the wire and nothing on the host, so even a batch
/// of one is cheaper than building the equivalent object field by field.
///
/// Layout (little endian), mirrored by `ts/wire-info.ts`:
/// ```text
/// header:      u32 records | u32 new_strings | u32 new_jids | u32 str_len | u32 flags
/// new strings: N x [u16 slot][u16 byte_len]
/// new jids:    M x [u16 slot][u16 user][u16 server][u8 agent][u16 device][u16 integrator]
/// records:     R x <per-type layout>
/// strings:     [cache definitions in slot order][inline values in record order]
/// ```
/// Cached slots are written as `u16` (optional slots carry slot + 1, 0 means
/// absent); unbounded values (message ids) are written inline with their
/// length in the record, so they never enter the cache.
#[derive(Default)]
pub(crate) struct FlatBatchWriter {
    string_cache: HashMap<String, u16>,
    jid_cache: HashMap<Jid, u16>,
    new_strings: Vec<u8>,
    new_jids: Vec<u8>,
    records: Vec<u8>,
    /// Bytes of the strings this batch defines into the cache.
    definitions: Vec<u8>,
    /// Bytes of this batch's inline values, in record order.
    inline: Vec<u8>,
    new_string_count: u32,
    new_jid_count: u32,
    record_count: u32,
    flags: u32,
    out: Vec<u8>,
}

const PACKED_HEADER_BYTES: usize = 20;

impl FlatBatchWriter {
    fn begin(&mut self) {
        if self.string_cache.len() >= PACKED_STRING_CACHE_MAX
            || self.jid_cache.len() >= PACKED_JID_CACHE_MAX
        {
            self.string_cache.clear();
            self.jid_cache.clear();
            self.flags |= PACKED_FLAG_RESET_CACHES;
        }
    }

    fn cache_str(&mut self, value: &str) -> u16 {
        if let Some(&slot) = self.string_cache.get(value) {
            return slot;
        }
        let slot = self.string_cache.len() as u16;
        self.string_cache.insert(value.to_owned(), slot);
        self.new_strings.extend_from_slice(&slot.to_le_bytes());
        self.new_strings
            .extend_from_slice(&(value.len() as u16).to_le_bytes());
        self.new_string_count += 1;
        // Definitions are emitted before any inline value, so appending keeps
        // the host's read order.
        self.definitions.extend_from_slice(value.as_bytes());
        slot
    }

    /// Optional slots carry slot + 1 so 0 can mean absent.
    fn cache_str_optional(&mut self, value: Option<&str>) -> u16 {
        match value {
            Some(value) => self.cache_str(value) + 1,
            None => 0,
        }
    }

    fn cache_jid(&mut self, jid: &Jid) -> u16 {
        if let Some(&slot) = self.jid_cache.get(jid) {
            return slot;
        }
        let user = self.cache_str(jid.user.as_str());
        let server = self.cache_str(jid.server.as_str());
        let slot = self.jid_cache.len() as u16;
        self.jid_cache.insert(jid.clone(), slot);
        self.new_jids.extend_from_slice(&slot.to_le_bytes());
        self.new_jids.extend_from_slice(&user.to_le_bytes());
        self.new_jids.extend_from_slice(&server.to_le_bytes());
        self.new_jids.push(jid.agent);
        self.new_jids.extend_from_slice(&jid.device.to_le_bytes());
        self.new_jids
            .extend_from_slice(&jid.integrator.to_le_bytes());
        self.new_jid_count += 1;
        slot
    }

    fn cache_jid_optional(&mut self, jid: Option<&Jid>) -> u16 {
        match jid {
            Some(jid) => self.cache_jid(jid) + 1,
            None => 0,
        }
    }

    #[inline]
    fn write_slot(&mut self, slot: u16) {
        self.records.extend_from_slice(&slot.to_le_bytes());
    }

    #[inline]
    fn write_f64(&mut self, value: f64) {
        self.records.extend_from_slice(&value.to_le_bytes());
    }

    /// Write an unbounded value: its byte length goes in the record, its bytes
    /// into the inline region.
    #[inline]
    fn write_inline(&mut self, value: &str) {
        self.records
            .extend_from_slice(&(value.len() as u16).to_le_bytes());
        self.inline.extend_from_slice(value.as_bytes());
    }

    fn finish(&mut self) -> Result<JsValue, JsValue> {
        self.definitions.extend_from_slice(&self.inline);
        self.out.clear();
        self.out.reserve(
            PACKED_HEADER_BYTES
                + self.new_strings.len()
                + self.new_jids.len()
                + self.records.len()
                + self.definitions.len(),
        );
        self.out.extend_from_slice(&self.record_count.to_le_bytes());
        self.out
            .extend_from_slice(&self.new_string_count.to_le_bytes());
        self.out
            .extend_from_slice(&self.new_jid_count.to_le_bytes());
        self.out
            .extend_from_slice(&(self.definitions.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&self.flags.to_le_bytes());
        self.out.extend_from_slice(&self.new_strings);
        self.out.extend_from_slice(&self.new_jids);
        self.out.extend_from_slice(&self.records);
        self.out.extend_from_slice(&self.definitions);

        let batch = js_sys::Object::new();
        js_keys::set(
            &batch,
            &js_keys::PACKED_BUFFER_KEY,
            &js_sys::Uint8Array::from(self.out.as_slice()).into(),
        )?;

        self.new_strings.clear();
        self.new_jids.clear();
        self.records.clear();
        self.definitions.clear();
        self.inline.clear();
        self.new_string_count = 0;
        self.new_jid_count = 0;
        self.record_count = 0;
        self.flags = 0;
        Ok(batch.into())
    }
}

/// Packed transport for `Event::Receipt` runs.
///
/// Record: `u8 flags | u16 x 8 cached slots | f64 timestamp | u8 id_count |
/// u16 x id_count inline lengths`.
#[derive(Default)]
pub(crate) struct ReceiptWireBatch {
    writer: FlatBatchWriter,
}

thread_local! {
    static RECEIPT_ENCODER: RefCell<ReceiptWireBatch> =
        RefCell::new(ReceiptWireBatch::default());
}

impl PackedEventBatch for ReceiptWireBatch {
    const KIND: &'static str = "receipt";

    fn accepts(event: &Event) -> bool {
        matches!(event, Event::Receipt(_))
    }

    fn with_encoder<R>(f: impl FnOnce(&mut Self) -> R) -> R {
        RECEIPT_ENCODER.with(|encoder| f(&mut encoder.borrow_mut()))
    }

    fn begin(&mut self) {
        self.writer.begin();
    }

    fn push(&mut self, event: &Event) -> Result<(), JsValue> {
        let Event::Receipt(receipt) = event else {
            return Err(JsValue::from_str("receipt batch received a foreign event"));
        };
        let source = &receipt.source;
        let (type_repr, type_other) = receipt_type_repr(&receipt.r#type);
        let mut flags = 0u8;
        if source.is_from_me {
            flags |= RECEIPT_FLAG_FROM_ME;
        }
        if source.is_group {
            flags |= RECEIPT_FLAG_GROUP;
        }
        if receipt.offline {
            flags |= RECEIPT_FLAG_OFFLINE;
        }
        if type_other {
            flags |= RECEIPT_FLAG_TYPE_OTHER;
        }

        let chat = self.writer.cache_jid(&source.chat);
        let sender = self.writer.cache_jid(&source.sender);
        let sender_alt = self.writer.cache_jid_optional(source.sender_alt.as_ref());
        let recipient_alt = self
            .writer
            .cache_jid_optional(source.recipient_alt.as_ref());
        let broadcast_list_owner = self
            .writer
            .cache_jid_optional(source.broadcast_list_owner.as_ref());
        let recipient = self.writer.cache_jid_optional(source.recipient.as_ref());
        let addressing_mode = self
            .writer
            .cache_str_optional(source.addressing_mode.as_ref().map(|mode| mode.as_str()));
        let type_slot = self.writer.cache_str(type_repr);

        self.writer.records.push(flags);
        for slot in [
            chat,
            sender,
            sender_alt,
            recipient_alt,
            broadcast_list_owner,
            recipient,
            addressing_mode,
            type_slot,
        ] {
            self.writer.write_slot(slot);
        }
        self.writer.write_f64(receipt.timestamp.timestamp() as f64);
        let id_count = u8::try_from(receipt.message_ids.len()).map_err(|_| {
            JsValue::from_str("receipt carries more message ids than the wire format holds")
        })?;
        self.writer.records.push(id_count);
        for id in &receipt.message_ids {
            self.writer.write_inline(id);
        }
        self.writer.record_count += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<JsValue, JsValue> {
        self.writer.finish()
    }
}

/// Packed transport for `Event::ServerAck` runs.
///
/// Record: `u16 class | u16 from | u16 error | f64 timestamp (NaN = absent) |
/// u16 inline id length`.
#[derive(Default)]
pub(crate) struct ServerAckWireBatch {
    writer: FlatBatchWriter,
}

thread_local! {
    static SERVER_ACK_ENCODER: RefCell<ServerAckWireBatch> =
        RefCell::new(ServerAckWireBatch::default());
}

impl PackedEventBatch for ServerAckWireBatch {
    const KIND: &'static str = "server_ack";

    fn accepts(event: &Event) -> bool {
        matches!(event, Event::ServerAck(_))
    }

    fn with_encoder<R>(f: impl FnOnce(&mut Self) -> R) -> R {
        SERVER_ACK_ENCODER.with(|encoder| f(&mut encoder.borrow_mut()))
    }

    fn begin(&mut self) {
        self.writer.begin();
    }

    fn push(&mut self, event: &Event) -> Result<(), JsValue> {
        let Event::ServerAck(ack) = event else {
            return Err(JsValue::from_str(
                "server-ack batch received a foreign event",
            ));
        };
        let class = self.writer.cache_str_optional(ack.class.as_deref());
        let from = self.writer.cache_jid_optional(ack.from.as_ref());
        let error = self.writer.cache_str_optional(ack.error.as_deref());
        self.writer.write_slot(class);
        self.writer.write_slot(from);
        self.writer.write_slot(error);
        // NaN marks an absent timestamp; a real one is always finite.
        self.writer
            .write_f64(ack.timestamp.map_or(f64::NAN, |t| t.timestamp() as f64));
        // Ack ids are unique per stanza, so they stay out of the cache.
        self.writer.write_inline(&ack.id);
        self.writer.record_count += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<JsValue, JsValue> {
        self.writer.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    use whatsapp_rust::wacore::types::events::InboundMessage;
    use whatsapp_rust::wacore::types::message::{EditAttribute, MessageInfo};
    use whatsapp_rust::waproto::whatsapp::Message;

    #[test]
    fn message_wire_batch_packs_canonical_records_and_dedups_strings() {
        let mut info = MessageInfo::default();
        info.source.chat = "120363@g.us".parse().expect("valid chat jid");
        info.source.sender = "5511:7@s.whatsapp.net".parse().expect("valid sender jid");
        info.source.sender_alt = Some("999:3@lid".parse().expect("valid alternate jid"));
        info.id = "WIRE-1".into();
        info.push_name = "Alice".into();
        info.edit = EditAttribute::MessageEdit;
        info.unavailable_request_id = Some("PDO-1".into());

        let mut second = MessageInfo::default();
        second.source.chat = "120363@g.us".parse().expect("valid chat jid");
        second.source.sender = "5511:7@s.whatsapp.net".parse().expect("valid sender jid");
        second.id = "WIRE-2".into();
        second.push_name = "Alice".into();

        let mut batch = MessageWireBatch::default();
        for info in [info, second] {
            let inbound = InboundMessage::builder()
                .message(Arc::new(Message::default()))
                .info(Arc::new(info))
                .build();
            batch.push(&inbound).expect("push packs the record");
        }
        assert_eq!(batch.len(), 2);
        assert_eq!(batch.info_records.len(), 2 * MESSAGE_WIRE_INFO_RECORD_WIDTH);

        let table: Vec<&str> = batch
            .strings
            .offsets
            .windows(2)
            .map(|w| {
                std::str::from_utf8(&batch.strings.data[w[0] as usize..w[1] as usize])
                    .expect("table entries are UTF-8")
            })
            .collect();
        // Canonical (non-AD) forms, deduplicated across both records.
        assert_eq!(
            table,
            [
                "120363@g.us",
                "5511@s.whatsapp.net",
                "999@lid",
                "WIRE-1",
                "Alice",
                "PDO-1",
                "1",
                "WIRE-2",
            ]
        );

        let record = &batch.info_records[..MESSAGE_WIRE_INFO_RECORD_WIDTH];
        assert_eq!(record[0], 0.0); // chat
        assert_eq!(record[1], 1.0); // sender
        assert_eq!(record[2], 3.0); // senderAlt (optional: index + 1)
        assert_eq!(record[3], 0.0); // recipientAlt absent
        assert_eq!(record[4], 3.0); // id
        assert_eq!(record[5], 4.0); // pushName
        assert_eq!(record[7], 0.0); // flags: none set
        assert_eq!(record[8], 6.0); // unavailableRequestId (index + 1)
        assert_eq!(record[9], 7.0); // edit "1" (index + 1)

        let second_record = &batch.info_records[MESSAGE_WIRE_INFO_RECORD_WIDTH..];
        assert_eq!(second_record[0], 0.0); // chat deduplicated
        assert_eq!(second_record[1], 1.0); // sender deduplicated
        assert_eq!(second_record[2], 0.0); // senderAlt absent
        assert_eq!(second_record[4], 7.0); // id "WIRE-2"
        assert_eq!(second_record[5], 4.0); // pushName deduplicated
    }
}
