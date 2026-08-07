//! Packed wire-batch transport: fixed-width numeric records plus a per-batch
//! deduplicated string table, so repeated addresses and metadata pay one
//! host-side decode per batch instead of one FFI object build per event.
//! Record layouts are mirrored by the host decoders in `ts/wire-info.ts`.

use std::cell::RefCell;
use std::collections::HashMap;

use wasm_bindgen::JsValue;
use whatsapp_rust::wacore;
use whatsapp_rust::wacore::types::events::Event;
use whatsapp_rust::wacore_binary::jid::{Jid, push_jid_to_string};
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
/// Six u32 fields: messages, strings, message bytes, string bytes, record width,
/// reserved. A multiple of 8 so the f64 record block that follows is aligned.
const MESSAGE_WIRE_HEADER_BYTES: usize = 24;
/// Payload capacity the reused encoder keeps between batches. Above it a batch
/// of large media protobufs would pin its own peak for the rest of the session.
const MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES: usize = 4 * crate::WASM_PAGE_BYTES;
const INFO_FLAG_FROM_ME: u32 = 1 << 0;
const INFO_FLAG_GROUP: u32 = 1 << 1;
const INFO_FLAG_VIEW_ONCE: u32 = 1 << 2;
const INFO_FLAG_OFFLINE: u32 = 1 << 3;

/// Per-batch deduplicated string table shared by every packed wire batch.
/// Repeated values (addresses, push names, ack classes) pay one host-side
/// decode per batch instead of one FFI string crossing per event.
///
/// Dedup scans the entries already written instead of indexing them in a map:
/// a batch holds at most `EVENT_BATCH_CAPACITY` messages, so the scan stays in
/// the tens of length comparisons, and a map would cost one owned `String` per
/// distinct value on a path that otherwise allocates nothing per message.
#[derive(Default)]
pub(crate) struct WireStringTable {
    /// Concatenated UTF-8 payloads of the table entries.
    data: Vec<u8>,
    /// K + 1 byte offsets delimiting the K entries (leading zero).
    offsets: Vec<u32>,
    /// Rendering buffer for values that have no `&str` form of their own,
    /// reused so rendering one does not allocate.
    scratch: String,
}

impl WireStringTable {
    fn intern(&mut self, value: &str) -> u32 {
        let value = value.as_bytes();
        for (index, window) in self.offsets.windows(2).enumerate() {
            if &self.data[window[0] as usize..window[1] as usize] == value {
                return index as u32;
            }
        }
        if self.offsets.is_empty() {
            self.offsets.push(0);
        }
        let index = (self.offsets.len() - 1) as u32;
        self.data.extend_from_slice(value);
        self.offsets.push(self.data.len() as u32);
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

    /// Intern a JID in its canonical (non-AD) form, rendered into the reusable
    /// scratch rather than into an owned `String` per call.
    fn intern_jid(&mut self, jid: &Jid) -> u32 {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        push_jid_to_string(&jid.user, jid.server, 0, 0, &mut scratch);
        let index = self.intern(&scratch);
        self.scratch = scratch;
        index
    }

    #[inline]
    fn intern_jid_optional(&mut self, jid: Option<&Jid>) -> f64 {
        match jid {
            Some(jid) => (self.intern_jid(jid) + 1) as f64,
            None => 0.0,
        }
    }

    /// Number of interned entries. The offset vector carries a leading zero, so
    /// it holds one more element than there are entries.
    #[inline]
    fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Drop the entries but keep every buffer, so the next batch interns into
    /// allocations this one already paid for.
    fn reset(&mut self) {
        self.data.clear();
        self.offsets.clear();
    }
}

/// Lay a batch out in the process-wide scratch and hand the bytes to `cross`.
///
/// The scratch is shared because `cross` finishes with the bytes before this
/// returns, so no batch can observe another's contents.
fn assemble_flat_batch<R>(write: impl FnOnce(&mut Vec<u8>), cross: impl FnOnce(&[u8]) -> R) -> R {
    thread_local! {
        static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }
    SCRATCH.with(|scratch| {
        let mut out = scratch.borrow_mut();
        out.clear();
        write(&mut out);
        cross(&out)
    })
}

/// Cross a batch the host may keep, as the bare `Uint8Array` every packed
/// transport shares: it gets its own `ArrayBuffer`.
///
/// Wrapping it in a `{ buffer }` object instead would cost an `Object::new` plus
/// a `Reflect::set` crossing per batch, and one message produces three batches
/// (its own, its receipt, its ack). The buffer IS the batch.
fn cross_owned_batch(write: impl FnOnce(&mut Vec<u8>)) -> JsValue {
    assemble_flat_batch(write, |bytes| js_sys::Uint8Array::from(bytes).into())
}

/// Bytes the shared host buffer keeps between batches. Beyond it a batch gets
/// its own allocation instead, so one outlier cannot pin a large buffer for the
/// rest of the session.
const SHARED_HOST_BUFFER_MAX_BYTES: u32 = crate::WASM_PAGE_BYTES as u32;

/// Cross a batch through one reused host buffer, returned as a subarray of it.
///
/// Sound only for batch kinds whose host decoder materialises every field
/// before returning: receipts and server acks do, so nothing the caller holds
/// still points here when the next batch overwrites it. The message batch hands
/// out payload views instead and keeps [`cross_owned_batch`].
///
/// The buffer is allocated by the host, not carved out of linear memory, so
/// growing WASM memory cannot detach it.
fn cross_shared_batch(write: impl FnOnce(&mut Vec<u8>)) -> JsValue {
    thread_local! {
        static HOST_BUFFER: RefCell<Option<js_sys::Uint8Array>> = const { RefCell::new(None) };
    }
    assemble_flat_batch(write, |bytes| {
        let len = bytes.len() as u32;
        if len > SHARED_HOST_BUFFER_MAX_BYTES {
            return js_sys::Uint8Array::from(bytes).into();
        }
        HOST_BUFFER.with(|cached| {
            let mut cached = cached.borrow_mut();
            let buffer = match cached.as_ref() {
                Some(buffer) if buffer.length() >= len => buffer,
                _ => cached.insert(js_sys::Uint8Array::new_with_length(
                    SHARED_HOST_BUFFER_MAX_BYTES,
                )),
            };
            let batch = buffer.subarray(0, len);
            batch.copy_from(bytes);
            batch.into()
        })
    })
}

/// Append a u32 offset table, materialising the leading zero sentinel that
/// `push` only writes once there is a first entry.
fn write_offsets(out: &mut Vec<u8>, offsets: &[u32]) {
    if offsets.is_empty() {
        out.extend_from_slice(&0u32.to_le_bytes());
        return;
    }
    for offset in offsets {
        out.extend_from_slice(&offset.to_le_bytes());
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

        let chat = self.strings.intern_jid(&source.chat) as f64;
        let sender = self.strings.intern_jid(&source.sender) as f64;
        let sender_alt = self.strings.intern_jid_optional(source.sender_alt.as_ref());
        let recipient_alt = self
            .strings
            .intern_jid_optional(source.recipient_alt.as_ref());
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

    /// Assemble the batch for the host and reset the per-batch state.
    ///
    /// Every buffer keeps its capacity: the encoder outlives the batch (see
    /// [`MessageWireBatch::with_encoder`]), so a steady stream of messages
    /// reuses the allocations the first batch paid for. A batch of unusually
    /// large payloads would otherwise pin its peak for the whole session, so
    /// the payload buffer is the one that gets trimmed back.
    #[cfg_attr(
        feature = "memory-profiling",
        tracing::instrument(name = "bridge.event.message_wire.ffi", level = "trace", skip_all)
    )]
    pub(crate) fn finish(&mut self) -> Result<JsValue, JsValue> {
        let batch = cross_owned_batch(|out| self.write_flat(out));
        self.reset();
        Ok(batch)
    }

    /// Drop the batch contents while keeping the buffers.
    pub(crate) fn reset(&mut self) {
        self.message_data.clear();
        if self.message_data.capacity() > MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES {
            self.message_data
                .shrink_to(MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES);
        }
        self.message_offsets.clear();
        self.info_records.clear();
        self.strings.reset();
    }

    /// Run `f` with the process-wide message encoder, so the buffers a batch
    /// needs are allocated once rather than once per dispatch. Sound because
    /// the batch bytes are copied into the JS typed array before the borrow
    /// ends: no host callback ever observes the encoder.
    pub(crate) fn with_encoder<R>(f: impl FnOnce(&mut Self) -> R) -> R {
        thread_local! {
            static MESSAGE_ENCODER: RefCell<MessageWireBatch> =
                RefCell::new(MessageWireBatch::default());
        }
        MESSAGE_ENCODER.with(|encoder| f(&mut encoder.borrow_mut()))
    }

    /// Serialise the batch into one contiguous buffer.
    ///
    /// The `f64` record block leads so it lands 8-aligned (the header is a
    /// multiple of 8) and the `u32` tables follow at 4-aligned offsets, which
    /// lets the host build typed-array views straight over the buffer. Padding
    /// is therefore never needed — a property the host-side decoder asserts.
    fn write_flat(&self, out: &mut Vec<u8>) {
        let header = [
            self.len() as u32,
            self.strings.len() as u32,
            self.message_data.len() as u32,
            self.strings.data.len() as u32,
            MESSAGE_WIRE_INFO_RECORD_WIDTH as u32,
            // Reserved: keeps the header a multiple of 8 for the record block.
            0,
        ];
        out.reserve(
            MESSAGE_WIRE_HEADER_BYTES
                + self.info_records.len() * 8
                + (self.message_offsets.len() + self.strings.offsets.len() + 2) * 4
                + self.message_data.len()
                + self.strings.data.len(),
        );
        for field in header {
            out.extend_from_slice(&field.to_le_bytes());
        }
        for record in &self.info_records {
            out.extend_from_slice(&record.to_le_bytes());
        }
        write_offsets(out, &self.message_offsets);
        write_offsets(out, &self.strings.offsets);
        out.extend_from_slice(&self.message_data);
        out.extend_from_slice(&self.strings.data);
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
        let batch = cross_shared_batch(|out| {
            out.reserve(
                PACKED_HEADER_BYTES
                    + self.new_strings.len()
                    + self.new_jids.len()
                    + self.records.len()
                    + self.definitions.len(),
            );
            out.extend_from_slice(&self.record_count.to_le_bytes());
            out.extend_from_slice(&self.new_string_count.to_le_bytes());
            out.extend_from_slice(&self.new_jid_count.to_le_bytes());
            out.extend_from_slice(&(self.definitions.len() as u32).to_le_bytes());
            out.extend_from_slice(&self.flags.to_le_bytes());
            out.extend_from_slice(&self.new_strings);
            out.extend_from_slice(&self.new_jids);
            out.extend_from_slice(&self.records);
            out.extend_from_slice(&self.definitions);
        });

        self.new_strings.clear();
        self.new_jids.clear();
        self.records.clear();
        self.definitions.clear();
        self.inline.clear();
        self.new_string_count = 0;
        self.new_jid_count = 0;
        self.record_count = 0;
        self.flags = 0;
        Ok(batch)
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

    /// Pins the byte layout the host decoder (`decodeMessageWireBatch`) reads.
    /// A divergence here would otherwise only surface end to end.
    #[test]
    fn message_wire_batch_writes_the_aligned_flat_layout() {
        let mut info = MessageInfo::default();
        info.source.chat = "120363@g.us".parse().expect("valid chat jid");
        info.source.sender = "5511:7@s.whatsapp.net".parse().expect("valid sender jid");
        info.id = "WIRE-1".into();
        info.push_name = "Alice".into();

        let mut batch = MessageWireBatch::default();
        let inbound = InboundMessage::builder()
            .message(Arc::new(Message::default()))
            .info(Arc::new(info))
            .build();
        batch.push(&inbound).expect("push packs the record");

        let mut out = Vec::new();
        batch.write_flat(&mut out);

        let u32_at = |offset: usize| {
            u32::from_le_bytes(out[offset..offset + 4].try_into().expect("4 bytes")) as usize
        };
        assert_eq!(u32_at(0), 1, "message count");
        let string_count = u32_at(4);
        assert_eq!(string_count, 4, "chat, sender, id, pushName");
        let message_bytes = u32_at(8);
        let string_bytes = u32_at(12);
        assert_eq!(u32_at(16), MESSAGE_WIRE_INFO_RECORD_WIDTH, "record width");
        assert_eq!(u32_at(20), 0, "reserved keeps the header 8-aligned");

        // Records lead so they land 8-aligned behind the header.
        assert_eq!(MESSAGE_WIRE_HEADER_BYTES % 8, 0);
        let records_end = MESSAGE_WIRE_HEADER_BYTES + MESSAGE_WIRE_INFO_RECORD_WIDTH * 8;
        let chat_slot = f64::from_le_bytes(
            out[MESSAGE_WIRE_HEADER_BYTES..MESSAGE_WIRE_HEADER_BYTES + 8]
                .try_into()
                .expect("8 bytes"),
        );
        assert_eq!(chat_slot, 0.0, "chat interns first");

        // Then the two offset tables, each carrying its leading sentinel.
        assert_eq!(u32_at(records_end), 0, "payload offset sentinel");
        assert_eq!(u32_at(records_end + 4), message_bytes, "payload end");
        let string_offsets_at = records_end + 8;
        assert_eq!(u32_at(string_offsets_at), 0, "string offset sentinel");

        let payloads_at = string_offsets_at + (string_count + 1) * 4;
        let strings_at = payloads_at + message_bytes;
        assert_eq!(out.len(), strings_at + string_bytes, "no trailing padding");
        assert_eq!(
            std::str::from_utf8(&out[strings_at..strings_at + string_bytes])
                .expect("table bytes are UTF-8"),
            "120363@g.us5511@s.whatsapp.netWIRE-1Alice"
        );
    }

    /// Builds a one-message batch whose payload is `payload_len` bytes of text.
    fn inbound(id: &str, payload_len: usize) -> InboundMessage {
        let mut info = MessageInfo::default();
        info.source.chat = "5511999@s.whatsapp.net".parse().expect("valid chat jid");
        info.source.sender = "5511999:9@s.whatsapp.net"
            .parse()
            .expect("valid sender jid");
        info.id = id.into();
        info.push_name = "Peer".into();
        let message = Message {
            conversation: Some("m".repeat(payload_len)),
            ..Default::default()
        };
        InboundMessage::builder()
            .message(Arc::new(message))
            .info(Arc::new(info))
            .build()
    }

    /// The crossed batch, as the host sees it.
    fn as_bytes(value: JsValue) -> js_sys::Uint8Array {
        use wasm_bindgen::JsCast;
        value.unchecked_into::<js_sys::Uint8Array>()
    }

    fn flat_of(batch: &MessageWireBatch) -> Vec<u8> {
        let mut out = Vec::new();
        batch.write_flat(&mut out);
        out
    }

    /// Happy path: a reused encoder is byte-identical to a fresh one, below the
    /// retained-payload threshold, at it, and above it.
    #[test]
    fn reused_encoder_is_byte_identical_to_a_fresh_one() {
        let sizes = [
            32,
            MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES,
            MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES * 2,
        ];
        for (round, size) in sizes.iter().copied().enumerate() {
            let message = inbound(&format!("SIZE-{round}"), size);

            let mut fresh = MessageWireBatch::default();
            fresh.push(&message).expect("fresh encoder packs");
            let expected = flat_of(&fresh);

            let reused = MessageWireBatch::with_encoder(|encoder| {
                encoder.push(&message).expect("reused encoder packs");
                let bytes = flat_of(encoder);
                encoder.reset();
                bytes
            });
            assert_eq!(reused, expected, "payload of {size} bytes");
        }
    }

    /// The encoder outlives the batch, so a batch abandoned before `finish`
    /// must not resurface in the next one.
    #[test]
    fn reset_drops_an_abandoned_batch() {
        let first = inbound("ABANDONED", 16);
        let second = inbound("KEPT", 16);
        MessageWireBatch::with_encoder(|encoder| {
            encoder.push(&first).expect("packs");
            encoder.reset();
            encoder.push(&second).expect("packs");
            assert_eq!(encoder.len(), 1);
            let bytes = flat_of(encoder);
            encoder.reset();
            let text = String::from_utf8_lossy(&bytes).into_owned();
            assert!(text.contains("KEPT"));
            assert!(!text.contains("ABANDONED"));
        });
    }

    /// The scan-based dedup must not confuse a value with a longer one that
    /// starts the same way.
    #[test]
    fn string_table_distinguishes_prefixes() {
        let mut table = WireStringTable::default();
        let a = table.intern("a");
        let ab = table.intern("ab");
        let a_again = table.intern("a");
        assert_eq!(a, 0);
        assert_eq!(ab, 1);
        assert_eq!(a_again, 0, "an exact repeat still dedups");
        assert_eq!(table.len(), 2);
        assert_eq!(table.data, b"aab");
    }

    /// The shared host buffer must not reach a batch the host may keep: two
    /// owned crossings stay independent.
    #[test]
    fn owned_batches_do_not_alias() {
        let first = cross_owned_batch(|out| out.extend_from_slice(&[1u8; 64]));
        let first = as_bytes(first);
        let _second = cross_owned_batch(|out| out.extend_from_slice(&[2u8; 64]));
        assert_eq!(
            first.to_vec(),
            vec![1u8; 64],
            "the first batch was rewritten"
        );
    }

    /// The invariant the receipt and server-ack decoders rely on: consecutive
    /// shared crossings reuse one host buffer, and each sees its own bytes.
    #[test]
    fn shared_batches_reuse_one_host_buffer() {
        let first = cross_shared_batch(|out| out.extend_from_slice(&[1u8; 64]));
        let first = as_bytes(first);
        let second = cross_shared_batch(|out| out.extend_from_slice(&[2u8; 48]));
        let second = as_bytes(second);

        assert_eq!(second.to_vec(), vec![2u8; 48]);
        assert_eq!(second.length(), 48, "the view is trimmed to the batch");
        assert!(
            js_sys::Object::is(&first.buffer().into(), &second.buffer().into()),
            "the buffer should be reused"
        );
    }

    /// The reason the shared buffer is host-allocated rather than a view into
    /// linear memory: growing WASM memory would detach the latter.
    #[test]
    fn shared_host_buffer_survives_linear_memory_growth() {
        let batch = cross_shared_batch(|out| out.extend_from_slice(&[7u8; 128]));
        let batch = as_bytes(batch);

        // Grow linear memory while the host still holds the batch.
        assert_ne!(
            core::arch::wasm32::memory_grow::<0>(16),
            usize::MAX,
            "memory.grow should succeed"
        );

        assert_eq!(batch.length(), 128, "the view was detached by the growth");
        assert_eq!(batch.to_vec(), vec![7u8; 128]);
    }

    /// A batch too large for the shared buffer gets its own, so an outlier
    /// neither pins a big buffer nor is clobbered by the next batch.
    #[test]
    fn oversized_batches_get_their_own_buffer() {
        let big = SHARED_HOST_BUFFER_MAX_BYTES as usize + 1;
        let oversized = cross_shared_batch(|out| out.extend_from_slice(&vec![9u8; big]));
        let oversized = as_bytes(oversized);
        let next = cross_shared_batch(|out| out.extend_from_slice(&[3u8; 32]));
        let next = as_bytes(next);

        assert_eq!(oversized.length() as usize, big);
        assert_eq!(
            oversized.get_index(0),
            9,
            "the oversized batch was rewritten"
        );
        assert!(!js_sys::Object::is(
            &oversized.buffer().into(),
            &next.buffer().into()
        ));
    }

    /// The encoder is process-wide, so the borrow must end before any host
    /// callback runs. Two batches built back to back must stay independent.
    #[test]
    fn consecutive_batches_are_serialized_and_independent() {
        let first = MessageWireBatch::with_encoder(|encoder| {
            encoder.push(&inbound("FIRST", 16)).expect("packs");
            encoder.finish().expect("crosses")
        });
        let first = as_bytes(first);
        let first_bytes = first.to_vec();

        let second = MessageWireBatch::with_encoder(|encoder| {
            assert!(encoder.is_empty(), "finish left state behind");
            encoder.push(&inbound("SECOND", 16)).expect("packs");
            encoder.finish().expect("crosses")
        });
        let second = as_bytes(second);

        assert_eq!(first.to_vec(), first_bytes, "the first batch was rewritten");
        let second_text = String::from_utf8_lossy(&second.to_vec()).into_owned();
        assert!(second_text.contains("SECOND"));
        assert!(!second_text.contains("FIRST"));
    }
}
