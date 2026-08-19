//! Packed wire-batch transport: fixed-width numeric records plus a string table
//! that outlives the batch, so a repeated address or push name is materialized
//! once on the host instead of once per batch — and never as one FFI object
//! build per event. Every transport here rolls that table the same way, under
//! `PACKED_FLAG_RESET_CACHES`. Record layouts are mirrored by the host decoders
//! in `ts/wire-info.ts`.

use std::cell::{Cell, RefCell};
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
    /// Segment tag this kind carries inside an event envelope.
    const SEGMENT_KIND: u32;
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
    fn finish(&mut self, buffer: BatchBuffer) -> Result<JsValue, JsValue>;
    /// Same bytes as [`PackedEventBatch::finish`], appended to `out` instead of
    /// crossed, so an envelope can carry the batch as one of its segments.
    fn write_and_reset(&mut self, out: &mut Vec<u8>);
    /// Records packed so far.
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Which host-side buffer a crossed batch lands in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BatchBuffer {
    /// A typed array of this batch's own, which the host may keep forever.
    Owned,
    /// The shared buffer, which the next batch overwrites. Only for hosts that
    /// opted into the synchronous borrow.
    Borrowed,
}

/// Slots per packed metadata record. Layout is mirrored by the host-side
/// decoder (`ts/wire-info.ts`); update both together.
pub(crate) const MESSAGE_WIRE_INFO_RECORD_WIDTH: usize = 10;
/// Six u32 fields: messages, new string definitions, message bytes, string
/// region bytes, record width, flags. A multiple of 8 so the f64 record block
/// that follows is aligned.
const MESSAGE_WIRE_HEADER_BYTES: usize = 24;
/// Payload capacity the reused encoder keeps between batches. Above it a batch
/// of large media protobufs would pin its own peak for the rest of the session.
const MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES: usize = 4 * crate::WASM_PAGE_BYTES;
/// Metadata capacity the reused string table keeps. A full batch of addresses,
/// ids and push names fits well inside it; the cap is what an outlier hits.
const WIRE_STRING_TABLE_RETAINED_BYTES: usize = 16 * 1024;
/// Bytes the message table's cross-batch cache may hold before it rolls. A push
/// name comes from a peer, so without this one oversized value would sit in the
/// table — and in the host's mirror of it — until the entry ceiling was reached.
const WIRE_STRING_CACHE_MAX_BYTES: usize = 64 * 1024;
/// Entry ceiling before a cross-batch table rolls, shared by every packed
/// transport. Bounded so a long session cannot grow the tables without limit;
/// a roll costs one batch of re-definitions.
const PACKED_STRING_CACHE_MAX: usize = 4096;
/// Header flag: the host must clear its table before reading this batch. The
/// one invalidation protocol both the message and the receipt/ack transports
/// speak — a table that outlives a batch is only safe while the two sides agree
/// on when it is dropped.
const PACKED_FLAG_RESET_CACHES: u32 = 1;
/// Header flag: the host must clear its table *after* reading this batch,
/// because the writer drops its own the moment this batch is out.
///
/// `PACKED_FLAG_RESET_CACHES` alone would say so only on the next batch, and on
/// a stream that then goes idle there is no next batch — the host would hold
/// everything the rolled table had, including a push name a peer sized, for as
/// long as the realm lives. This says it in the batch that causes the roll.
const PACKED_FLAG_CLEAR_AFTER: u32 = 1 << 1;
/// Size of the single host-allocated buffer offered to hosts that opted into
/// borrowing, and therefore the whole resident cost of the opt-in. A full
/// `EVENT_BATCH_CAPACITY` run of receipts packs into roughly 2 KiB, so this
/// holds every realistic batch; anything larger gets its own typed array.
const BORROWED_BATCH_BUFFER_BYTES: usize = 8 * 1024;
const INFO_FLAG_FROM_ME: u32 = 1 << 0;
const INFO_FLAG_GROUP: u32 = 1 << 1;
const INFO_FLAG_VIEW_ONCE: u32 = 1 << 2;
const INFO_FLAG_OFFLINE: u32 = 1 << 3;

/// Cross-batch string table for the message transport, holding the values the
/// host has already materialized. Addresses, push names and edit attributes
/// repeat batch after batch, so a batch defines only what the host does not
/// have yet and every repeat costs one numeric slot and nothing on the host.
///
/// Two rules keep the two sides in step, and both are the ones the packed
/// receipt/ack writer already follows:
///
/// - a definition is only counted as held once the batch carrying it has been
///   written out, so a batch the host never receives rolls the table instead of
///   leaving it one entry ahead ([`WireStringTable::abandon`]);
/// - a roll is announced, never inferred: the next batch carries
///   [`PACKED_FLAG_RESET_CACHES`] and re-defines what it needs.
///
/// Values that do not repeat — message ids, unavailable-request ids — are
/// written inline with their length in the record instead, so a stream of
/// unique ids can neither fill the table nor be retained by it.
pub(crate) struct WireStringTable {
    /// Table index of every value the host holds, by value.
    cache: HashMap<String, u32>,
    /// Entries the host holds, and therefore the index the next definition
    /// takes. Bounded by `PACKED_STRING_CACHE_MAX`.
    held: u32,
    /// Bytes `cache` holds, against `WIRE_STRING_CACHE_MAX_BYTES`.
    cache_bytes: usize,
    /// Concatenated UTF-8 payloads of the definitions this batch adds.
    definitions: Vec<u8>,
    /// K + 1 byte offsets delimiting this batch's K definitions (leading zero).
    definition_offsets: Vec<u32>,
    /// Definitions written so far, so a definition knows its index before the
    /// batch is committed.
    defined: u32,
    /// This batch's inline values, in record order. Their lengths ride in the
    /// records, so they need no offset table and never enter the cache.
    inline: Vec<u8>,
    /// Rendering buffer for values that have no `&str` form of their own,
    /// reused so rendering one does not allocate.
    scratch: String,
    /// Header flags the batch being built will carry.
    flags: u32,
}

impl Default for WireStringTable {
    /// A table with nothing in it still asks the host to clear: the encoder is
    /// per WASM instance and the host's mirror is per JS realm, so a fresh
    /// encoder cannot assume the mirror it is about to index is empty.
    fn default() -> Self {
        Self {
            cache: HashMap::new(),
            held: 0,
            cache_bytes: 0,
            definitions: Vec::new(),
            definition_offsets: Vec::new(),
            defined: 0,
            inline: Vec::new(),
            scratch: String::new(),
            flags: PACKED_FLAG_RESET_CACHES,
        }
    }
}

impl WireStringTable {
    /// Slot for a value the host may already hold, defining it if not.
    fn cache(&mut self, value: &str) -> u32 {
        if let Some(&index) = self.cache.get(value) {
            return index;
        }
        let index = self.held + self.defined;
        self.cache.insert(value.to_owned(), index);
        self.cache_bytes += value.len();
        if self.definition_offsets.is_empty() {
            self.definition_offsets.push(0);
        }
        self.definitions.extend_from_slice(value.as_bytes());
        self.definition_offsets.push(self.definitions.len() as u32);
        self.defined += 1;
        index
    }

    /// Optional slots carry index + 1 so 0 can mean absent.
    #[inline]
    fn cache_optional(&mut self, value: Option<&str>) -> f64 {
        match value {
            Some(value) => (self.cache(value) + 1) as f64,
            None => 0.0,
        }
    }

    /// Cache a JID in its canonical (non-AD) form, rendered into the reusable
    /// scratch rather than into an owned `String` per call. Only a value the
    /// table has not seen before is copied.
    fn cache_jid(&mut self, jid: &Jid) -> u32 {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        push_jid_to_string(&jid.user, jid.server, 0, 0, &mut scratch);
        let index = self.cache(&scratch);
        self.scratch = scratch;
        index
    }

    #[inline]
    fn cache_jid_optional(&mut self, jid: Option<&Jid>) -> f64 {
        match jid {
            Some(jid) => (self.cache_jid(jid) + 1) as f64,
            None => 0.0,
        }
    }

    /// Write a value that does not repeat. The record carries its byte length,
    /// the bytes go to the inline region in record order.
    #[inline]
    fn write_inline(&mut self, value: &str) -> f64 {
        self.inline.extend_from_slice(value.as_bytes());
        value.len() as f64
    }

    /// Optional inline slots carry length + 1 so 0 can mean absent — an empty
    /// value is not the same as no value.
    #[inline]
    fn write_inline_optional(&mut self, value: Option<&str>) -> f64 {
        match value {
            Some(value) => self.write_inline(value) + 1.0,
            None => 0.0,
        }
    }

    /// Definitions this batch adds to the host's table.
    #[inline]
    fn defined(&self) -> u32 {
        self.defined
    }

    /// Bytes this batch's string region spans: definitions then inline values.
    #[inline]
    fn region_len(&self) -> usize {
        self.definitions.len() + self.inline.len()
    }

    /// Clear the batch's own buffers, keeping the cache and the flags.
    ///
    /// The buffers a peer can inflate are capped: push names and message ids
    /// arrive from the wire, and these buffers outlive the batch, so one
    /// oversized event would otherwise pin its peak for the session.
    /// `definition_offsets` needs no cap, being bounded by the entry ceiling.
    fn clear_batch(&mut self) {
        for buffer in [&mut self.definitions, &mut self.inline] {
            buffer.clear();
            if buffer.capacity() > WIRE_STRING_TABLE_RETAINED_BYTES {
                buffer.shrink_to(WIRE_STRING_TABLE_RETAINED_BYTES);
            }
        }
        self.definition_offsets.clear();
        self.defined = 0;
        self.scratch.clear();
        if self.scratch.capacity() > WIRE_STRING_TABLE_RETAINED_BYTES {
            self.scratch.shrink_to(WIRE_STRING_TABLE_RETAINED_BYTES);
        }
    }

    /// Whether committing the batch as it stands will roll the table. Known
    /// before the header is written, which is what lets that header carry
    /// [`PACKED_FLAG_CLEAR_AFTER`] rather than leave the host waiting for a
    /// batch that may never come.
    fn will_roll(&self) -> bool {
        self.cache.len() >= PACKED_STRING_CACHE_MAX
            || self.cache_bytes >= WIRE_STRING_CACHE_MAX_BYTES
            || packed_tables_revoked()
    }

    /// Clear the cache and arm the flag that tells the host to do the same.
    fn roll(&mut self) {
        self.cache.clear();
        self.held = 0;
        self.cache_bytes = 0;
        self.flags |= PACKED_FLAG_RESET_CACHES;
    }

    /// The batch reached the host: its definitions are now held, and the flag
    /// it carried has been spent. A table at its ceiling rolls here rather than
    /// mid-batch, so the announcement rides the next batch's header.
    fn commit(&mut self) {
        let rolling = self.will_roll();
        self.held += self.defined;
        self.clear_batch();
        self.flags = 0;
        if rolling {
            self.roll();
        }
    }

    /// The batch was dropped before the host saw it. Its definitions took table
    /// indices that now exist on this side only, so the table has to roll:
    /// keeping them would have every later record point past what the host
    /// holds, and read a neighbour's value rather than fail.
    fn abandon(&mut self) {
        let defined = self.defined > 0;
        self.clear_batch();
        if defined {
            self.roll();
        }
    }
}

thread_local! {
    /// The one buffer every borrowed batch is handed a window on.
    ///
    /// Allocated on the host heap, never a view into linear memory: the host
    /// callback re-enters WASM by design, and a `memory.grow` there would
    /// detach such a view mid-batch.
    static SHARED_BORROW_BUFFER: RefCell<Option<js_sys::Uint8Array>> = const { RefCell::new(None) };
    /// Set when a borrowing host was seen breaking the synchronous contract.
    ///
    /// It lives with the buffer rather than with a callback, a batch kind or a
    /// client, because the buffer does: a window one host kept has to be safe
    /// from every later batch, whoever produces it.
    static BORROW_REVOKED: Cell<bool> = const { Cell::new(false) };
    /// Set when a host was seen deferring a batch past its callback.
    static PACKED_TABLES_REVOKED: Cell<bool> = const { Cell::new(false) };
}

/// Stop letting a table outlive its batch, for the rest of the process.
///
/// A table spanning batches is only sound in delivery order, and a host that
/// hands back something promise-like has not decoded inside its callback: its
/// decodes happen later, in an order the writer cannot see. So every batch
/// goes back to carrying `PACKED_FLAG_RESET_CACHES` and every value it
/// references, which is self-contained and therefore reads the same whenever
/// and in whatever order it is decoded.
///
/// Permanent, for the reason [`revoke_borrowed_batches`] is: nothing can tell
/// when such a host is finished, so resuming is a guess, and the cost of
/// guessing wrong is silent corruption against an optimization.
pub(crate) fn revoke_packed_tables() {
    if PACKED_TABLES_REVOKED.with(|revoked| revoked.replace(true)) {
        return;
    }
    invalidate_packed_tables();
}

pub(crate) fn packed_tables_revoked() -> bool {
    PACKED_TABLES_REVOKED.with(Cell::get)
}

/// Test-only, for the reason [`reset_borrowed_batches`] is.
#[cfg(test)]
pub(crate) fn reset_packed_tables() {
    PACKED_TABLES_REVOKED.with(|revoked| revoked.set(false));
}

/// Stop handing out windows on the shared buffer, for the rest of the process,
/// and abandon the buffer itself.
///
/// Dropping it is what makes the window the offending host kept unreachable no
/// matter what happens next, rather than merely unwritten by a flag someone
/// could later scope wrong. The flag then only decides whether to keep paying
/// for reuse, and it stays set: nothing can tell when a host that deferred its
/// decode is finished, so a later borrow is a guess, and the cost of guessing
/// wrong is silent corruption against an optimization worth a few percent.
pub(crate) fn revoke_borrowed_batches() {
    BORROW_REVOKED.with(|revoked| revoked.set(true));
    SHARED_BORROW_BUFFER.with(|shared| shared.borrow_mut().take());
}

pub(crate) fn borrowed_batches_revoked() -> bool {
    BORROW_REVOKED.with(Cell::get)
}

/// Test-only: the flag is permanent by design, so tests that exercise a
/// violation would otherwise disarm every test that runs after them.
#[cfg(test)]
pub(crate) fn reset_borrowed_batches() {
    BORROW_REVOKED.with(|revoked| revoked.set(false));
}

/// Assemble a flat batch and cross it as the bare `Uint8Array` every packed
/// transport shares, whatever the record layout inside is.
///
/// Wrapping it in a `{ buffer }` object instead would cost an `Object::new` plus
/// a `Reflect::set` crossing per batch, and a live message still produces up to
/// three batches (its own, its receipt, its ack) for a host that does not opt
/// into [`EventWireEnvelope`]. The buffer IS the batch.
///
/// The scratch is process-wide because its bytes are copied into the JS typed
/// array before this returns, so no batch can observe another's contents.
///
/// [`BatchBuffer::Borrowed`] hands out a window on one shared host buffer
/// instead, which the next batch overwrites. Only a host that registered a
/// borrowing callback ever sees it, and that callback's contract is what makes
/// the reuse safe: the window is dead the moment it returns.
fn cross_flat_batch(buffer: BatchBuffer, write: impl FnOnce(&mut Vec<u8>)) -> JsValue {
    thread_local! {
        static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }
    SCRATCH.with(|scratch| {
        let mut out = scratch.borrow_mut();
        out.clear();
        write(&mut out);
        let batch = cross_bytes(buffer, &out);
        if out.capacity() > MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES {
            // `shrink_to` never goes below the length, so drop it first.
            out.clear();
            out.shrink_to(MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES);
        }
        batch
    })
}

/// Hand `bytes` to the host in the buffer `buffer` names. Split from
/// [`cross_flat_batch`] so a batch already assembled elsewhere (an envelope's
/// own buffer) crosses through the same borrow rules without a second copy.
fn cross_bytes(buffer: BatchBuffer, bytes: &[u8]) -> JsValue {
    let batch = match buffer {
        // An outlier gets its own typed array, so it neither pins a larger
        // shared buffer nor loses its tail to the next batch.
        BatchBuffer::Borrowed
            if !borrowed_batches_revoked() && bytes.len() <= BORROWED_BATCH_BUFFER_BYTES =>
        {
            SHARED_BORROW_BUFFER.with(|shared| {
                let mut shared = shared.borrow_mut();
                let shared = shared.get_or_insert_with(|| {
                    js_sys::Uint8Array::new_with_length(BORROWED_BATCH_BUFFER_BYTES as u32)
                });
                let window = shared.subarray(0, bytes.len() as u32);
                window.copy_from(bytes);
                window
            })
        }
        _ => js_sys::Uint8Array::from(bytes),
    };
    batch.into()
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
/// numeric records plus a string table that outlives the batch: an address or a
/// push name is decoded once on the host and referenced by index for as long as
/// the table stands, instead of being materialized again every batch.
#[derive(Default)]
pub(crate) struct MessageWireBatch {
    /// Concatenated `proto.Message` payloads in event order.
    message_data: Vec<u8>,
    /// N + 1 byte offsets delimiting the N payloads in `messageData`. The
    /// leading zero sentinel makes empty and singleton batches unambiguous.
    message_offsets: Vec<u32>,
    strings: WireStringTable,
    /// `MESSAGE_WIRE_INFO_RECORD_WIDTH` slots per message. A cached slot holds
    /// a table index (optional slots are index + 1, 0 = absent); an inline slot
    /// holds its value's byte length (optional slots are length + 1).
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

        let chat = self.strings.cache_jid(&source.chat) as f64;
        let sender = self.strings.cache_jid(&source.sender) as f64;
        let sender_alt = self.strings.cache_jid_optional(source.sender_alt.as_ref());
        let recipient_alt = self
            .strings
            .cache_jid_optional(source.recipient_alt.as_ref());
        // A push name repeats across a batch and across batches alike, and a
        // peer-sized one repeated 32 times is exactly what the table is for:
        // one copy, however long it is. What bounds it on the host's side is
        // the byte ceiling and `PACKED_FLAG_CLEAR_AFTER`, not a length limit
        // here — writing it inline would cost a copy per message.
        let push_name = self.strings.cache(&info.push_name) as f64;
        let edit = self
            .strings
            .cache_optional((!edit.is_empty()).then_some(edit));
        // Inline values are written in the order the host reads them back: id,
        // then the request id. Ids are unique per message, so caching them
        // would fill the table with values no later batch can reference.
        let id = self.strings.write_inline(&info.id);
        let unavailable_request_id = self
            .strings
            .write_inline_optional(info.unavailable_request_id.as_deref());

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

    /// Assemble the batch for the host and clear the per-batch state.
    ///
    /// Every buffer keeps its capacity: the encoder outlives the batch (see
    /// [`MessageWireBatch::with_encoder`]), so a steady stream of messages
    /// reuses the allocations the first batch paid for. A batch of unusually
    /// large payloads would otherwise pin its peak for the whole session, so
    /// the payload buffer is the one that gets trimmed back.
    ///
    /// The batch always crosses in a buffer of its own. `decodeMessageWireBatch`
    /// hands the host `Uint8Array`/`Uint32Array` views over it rather than
    /// copies, so a borrowed buffer would alias whatever the host keeps.
    #[cfg_attr(
        feature = "memory-profiling",
        tracing::instrument(name = "bridge.event.message_wire.ffi", level = "trace", skip_all)
    )]
    pub(crate) fn finish(&mut self) -> Result<JsValue, JsValue> {
        let batch = cross_flat_batch(BatchBuffer::Owned, |out| self.write_flat(out));
        self.commit();
        Ok(batch)
    }

    /// Same bytes as [`MessageWireBatch::finish`], appended to `out` instead of
    /// crossed, so an envelope can carry the batch as one of its segments.
    pub(crate) fn write_and_reset(&mut self, out: &mut Vec<u8>) {
        self.write_flat(out);
        self.commit();
    }

    /// Drop the records while keeping the buffers.
    fn clear_records(&mut self) {
        self.message_data.clear();
        if self.message_data.capacity() > MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES {
            self.message_data
                .shrink_to(MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES);
        }
        self.message_offsets.clear();
        self.info_records.clear();
    }

    /// The batch was written out, so the host now holds what it defined.
    fn commit(&mut self) {
        self.clear_records();
        self.strings.commit();
    }

    /// Drop a batch the host will never see. Anything it defined into the
    /// string table goes with it — see [`WireStringTable::abandon`].
    pub(crate) fn reset(&mut self) {
        self.clear_records();
        self.strings.abandon();
    }

    /// Drop the batch and roll the table unconditionally, for when a batch that
    /// was already written out never reached the host — see
    /// [`invalidate_packed_tables`].
    fn invalidate(&mut self) {
        self.clear_records();
        self.strings.clear_batch();
        self.strings.roll();
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
    ///
    /// The string region is this batch's definitions followed by its inline
    /// values; the offset table delimits the definitions, so where they end is
    /// where the inline values begin.
    fn write_flat(&self, out: &mut Vec<u8>) {
        let header = [
            self.len() as u32,
            self.strings.defined(),
            self.message_data.len() as u32,
            self.strings.region_len() as u32,
            MESSAGE_WIRE_INFO_RECORD_WIDTH as u32,
            // The roll this batch is about to cause is announced in its own
            // header, so the host drops the table with it rather than holding
            // it until whenever the next batch arrives.
            self.strings.flags
                | if self.strings.will_roll() {
                    PACKED_FLAG_CLEAR_AFTER
                } else {
                    0
                },
        ];
        out.reserve(
            MESSAGE_WIRE_HEADER_BYTES
                + self.info_records.len() * 8
                + (self.message_offsets.len() + self.strings.definition_offsets.len() + 2) * 4
                + self.message_data.len()
                + self.strings.region_len(),
        );
        for field in header {
            out.extend_from_slice(&field.to_le_bytes());
        }
        for record in &self.info_records {
            out.extend_from_slice(&record.to_le_bytes());
        }
        write_offsets(out, &self.message_offsets);
        write_offsets(out, &self.strings.definition_offsets);
        out.extend_from_slice(&self.message_data);
        out.extend_from_slice(&self.strings.definitions);
        out.extend_from_slice(&self.strings.inline);
    }
}

const PACKED_JID_CACHE_MAX: usize = 1024;

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

    fn finish(&mut self, buffer: BatchBuffer) -> Result<JsValue, JsValue> {
        Ok(cross_flat_batch(buffer, |out| self.write_and_reset(out)))
    }

    fn write_and_reset(&mut self, out: &mut Vec<u8>) {
        self.definitions.extend_from_slice(&self.inline);
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

        self.clear_batch();
        self.flags = 0;
        if packed_tables_revoked() {
            self.string_cache.clear();
            self.jid_cache.clear();
            self.flags = PACKED_FLAG_RESET_CACHES;
        }
    }

    /// Drop the batch's own buffers, keeping the caches and the flags.
    fn clear_batch(&mut self) {
        self.new_strings.clear();
        self.new_jids.clear();
        self.records.clear();
        self.definitions.clear();
        self.inline.clear();
        self.new_string_count = 0;
        self.new_jid_count = 0;
        self.record_count = 0;
    }

    /// Drop a batch the host will never see, and roll the caches with it: the
    /// slots it took exist only here, so every later record would index past
    /// what the host holds. Announced through the same header flag a ceiling
    /// roll uses.
    fn invalidate(&mut self) {
        self.clear_batch();
        self.string_cache.clear();
        self.jid_cache.clear();
        self.flags |= PACKED_FLAG_RESET_CACHES;
    }
}

/// Segment tags carried by an event envelope. Mirrored by `ts/wire-info.ts`.
pub(crate) const EVENT_SEGMENT_KIND_MESSAGE: u32 = 1;
pub(crate) const EVENT_SEGMENT_KIND_RECEIPT: u32 = 2;
pub(crate) const EVENT_SEGMENT_KIND_SERVER_ACK: u32 = 3;

/// `u32 segment_count | u32 reserved`. Eight bytes so the first segment payload
/// lands 8-aligned behind its own 8-byte prefix.
const EVENT_ENVELOPE_HEADER_BYTES: usize = 8;
/// `u32 kind | u32 byte_len` ahead of each segment payload.
const EVENT_ENVELOPE_SEGMENT_PREFIX_BYTES: usize = 8;

/// What an accumulated run crosses as.
pub(crate) enum CrossedBatch {
    /// A lone batch crosses as the bare per-kind buffer it has always been, so
    /// a host only ever sees an envelope when the envelope saved a crossing.
    Single { kind: u32, batch: JsValue },
    /// Two or more batches, packed into one buffer of tagged segments.
    Envelope(JsValue),
}

/// Buffers the batches an adjacent run of events produces so the whole run
/// crosses once instead of once per batch.
///
/// A segment is byte for byte the buffer its per-kind callback would have
/// received, which is what lets the host decode it with the codec it already
/// has. Payloads stay 8-aligned so a message segment is still read as a view
/// over the envelope rather than copied out of it.
///
/// Layout (little endian), mirrored by `ts/wire-info.ts`:
/// ```text
/// header:  u32 segment_count | u32 reserved
/// segment: u32 kind | u32 byte_len | byte_len bytes | padding to 8
/// ```
#[derive(Default)]
pub(crate) struct EventWireEnvelope {
    buffer: Vec<u8>,
    /// `(kind, payload start, payload length)` per buffered segment.
    segments: Vec<(u32, usize, usize)>,
    records: usize,
}

impl EventWireEnvelope {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Events packed across every buffered segment.
    #[inline]
    pub(crate) fn records(&self) -> usize {
        self.records
    }

    /// The kind of the only buffered segment, when the run produced one. The
    /// caller needs it before crossing, to pick that kind's buffer.
    pub(crate) fn lone_segment_kind(&self) -> Option<u32> {
        match self.segments[..] {
            [(kind, _, _)] => Some(kind),
            _ => None,
        }
    }

    /// Append one finished batch as a tagged segment.
    pub(crate) fn push_segment(
        &mut self,
        kind: u32,
        records: usize,
        write: impl FnOnce(&mut Vec<u8>),
    ) {
        if self.buffer.is_empty() {
            self.buffer.resize(EVENT_ENVELOPE_HEADER_BYTES, 0);
        }
        self.buffer.reserve(EVENT_ENVELOPE_SEGMENT_PREFIX_BYTES);
        self.buffer.extend_from_slice(&kind.to_le_bytes());
        let length_at = self.buffer.len();
        self.buffer.extend_from_slice(&0u32.to_le_bytes());
        let start = self.buffer.len();
        write(&mut self.buffer);
        let length = self.buffer.len() - start;
        self.buffer[length_at..length_at + 4].copy_from_slice(&(length as u32).to_le_bytes());
        self.buffer.resize(self.buffer.len().next_multiple_of(8), 0);
        self.segments.push((kind, start, length));
        self.records += records;
    }

    /// Cross the buffered run in `buffer` and drop it, keeping the buffers.
    pub(crate) fn finish(&mut self, buffer: BatchBuffer) -> CrossedBatch {
        debug_assert!(!self.is_empty(), "finishing an empty envelope");
        let crossed = if let [(kind, start, length)] = self.segments[..] {
            CrossedBatch::Single {
                kind,
                batch: cross_bytes(buffer, &self.buffer[start..start + length]),
            }
        } else {
            let count = self.segments.len() as u32;
            self.buffer[..4].copy_from_slice(&count.to_le_bytes());
            CrossedBatch::Envelope(cross_bytes(buffer, &self.buffer))
        };
        self.reset();
        crossed
    }

    /// Drop the run while keeping the buffers, capped the way the crossing
    /// scratch is: one oversized run must not pin its peak for the session.
    pub(crate) fn reset(&mut self) {
        self.buffer.clear();
        if self.buffer.capacity() > MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES {
            self.buffer.shrink_to(MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES);
        }
        self.segments.clear();
        self.records = 0;
    }
}

impl Drop for EventWireEnvelope {
    /// A buffered run that is still here has not crossed, and the segments in
    /// it were written by encoders that already counted their definitions as
    /// held. Nothing else can see that: the dispatch future can be dropped at
    /// any suspension point, and it takes the envelope with it. So roll every
    /// table rather than reason about which kind the lost segments were.
    fn drop(&mut self) {
        if !self.segments.is_empty() {
            invalidate_packed_tables();
        }
    }
}

/// Roll every packed transport's table, announcing it to the host through
/// `PACKED_FLAG_RESET_CACHES`. The tables outlive the batch, so anything built
/// and then not delivered has to be taken back on both sides at once.
pub(crate) fn invalidate_packed_tables() {
    MessageWireBatch::with_encoder(MessageWireBatch::invalidate);
    RECEIPT_ENCODER.with(|encoder| encoder.borrow_mut().writer.invalidate());
    SERVER_ACK_ENCODER.with(|encoder| encoder.borrow_mut().writer.invalidate());
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
    const SEGMENT_KIND: u32 = EVENT_SEGMENT_KIND_RECEIPT;

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

        // Validated BEFORE the first byte goes out, and that ordering is the
        // point. `records` is one flat buffer the reader walks sequentially
        // against the header's `record_count`; bytes from a record that failed
        // halfway are not a lost record — the reader takes them as the start of
        // the next one and everything after decodes shifted. Failing here
        // leaves the batch untouched, so a rejected receipt costs only itself.
        //
        // u16, not u8. A read receipt aggregates every message it acknowledges,
        // and an active group clears well past 255 in one go, so the old ceiling
        // was reachable in ordinary use — and reaching it took the batch with it.
        let id_count = u16::try_from(receipt.message_ids.len()).map_err(|_| {
            JsValue::from_str("receipt carries more message ids than the wire format holds")
        })?;

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
        self.writer.write_slot(id_count);
        for id in &receipt.message_ids {
            self.writer.write_inline(id);
        }
        self.writer.record_count += 1;
        Ok(())
    }

    fn finish(&mut self, buffer: BatchBuffer) -> Result<JsValue, JsValue> {
        self.writer.finish(buffer)
    }

    fn write_and_reset(&mut self, out: &mut Vec<u8>) {
        self.writer.write_and_reset(out);
    }

    fn len(&self) -> usize {
        self.writer.record_count as usize
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
    const SEGMENT_KIND: u32 = EVENT_SEGMENT_KIND_SERVER_ACK;

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

    fn finish(&mut self, buffer: BatchBuffer) -> Result<JsValue, JsValue> {
        self.writer.finish(buffer)
    }

    fn write_and_reset(&mut self, out: &mut Vec<u8>) {
        self.writer.write_and_reset(out);
    }

    fn len(&self) -> usize {
        self.writer.record_count as usize
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

    /// The table's entries, in slot order, as the host would materialize them.
    fn definitions_of(batch: &MessageWireBatch) -> Vec<&str> {
        batch
            .strings
            .definition_offsets
            .windows(2)
            .map(|w| {
                std::str::from_utf8(&batch.strings.definitions[w[0] as usize..w[1] as usize])
                    .expect("definitions are UTF-8")
            })
            .collect()
    }

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

        // Canonical (non-AD) forms, deduplicated across both records. The ids
        // are not here: they are unique per message and go out inline.
        assert_eq!(
            definitions_of(&batch),
            [
                "120363@g.us",
                "5511@s.whatsapp.net",
                "999@lid",
                "Alice",
                "1"
            ]
        );
        assert_eq!(batch.strings.inline, b"WIRE-1PDO-1WIRE-2");

        let record = &batch.info_records[..MESSAGE_WIRE_INFO_RECORD_WIDTH];
        assert_eq!(record[0], 0.0); // chat
        assert_eq!(record[1], 1.0); // sender
        assert_eq!(record[2], 3.0); // senderAlt (optional: index + 1)
        assert_eq!(record[3], 0.0); // recipientAlt absent
        assert_eq!(record[4], 6.0); // id: inline byte length of "WIRE-1"
        assert_eq!(record[5], 3.0); // pushName
        assert_eq!(record[7], 0.0); // flags: none set
        assert_eq!(record[8], 6.0); // unavailableRequestId: length + 1
        assert_eq!(record[9], 5.0); // edit "1" (index + 1)

        let second_record = &batch.info_records[MESSAGE_WIRE_INFO_RECORD_WIDTH..];
        assert_eq!(second_record[0], 0.0); // chat deduplicated
        assert_eq!(second_record[1], 1.0); // sender deduplicated
        assert_eq!(second_record[2], 0.0); // senderAlt absent
        assert_eq!(second_record[4], 6.0); // id "WIRE-2" inline
        assert_eq!(second_record[5], 3.0); // pushName deduplicated
        assert_eq!(second_record[8], 0.0); // unavailableRequestId absent
    }

    /// The point of the change: a second batch from the same chat defines
    /// nothing and still addresses everything the first one defined.
    #[test]
    fn a_second_batch_reuses_the_table_the_first_one_defined() {
        let mut batch = MessageWireBatch::default();
        batch.push(&inbound("FIRST", 8)).expect("packs");
        let first_definitions: Vec<String> = definitions_of(&batch)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let first_record = batch.info_records.clone();
        // `inbound` addresses one contact, so chat and sender share a canonical
        // form: two entries, not three.
        assert_eq!(first_definitions, ["5511999@s.whatsapp.net", "Peer"]);
        assert_eq!(
            batch.strings.flags, PACKED_FLAG_RESET_CACHES,
            "a table nobody has yet still has to be claimed"
        );

        let mut out = Vec::new();
        batch.write_and_reset(&mut out);

        batch.push(&inbound("SECOND", 8)).expect("packs");
        assert!(
            definitions_of(&batch).is_empty(),
            "the second batch redefined the table"
        );
        assert_eq!(batch.strings.flags, 0, "nothing asked the host to clear");
        assert_eq!(batch.strings.inline, b"SECOND");
        // Same slots, so the host reads them out of the table it already holds.
        let second_record = &batch.info_records;
        for slot in [0usize, 1, 5] {
            assert_eq!(second_record[slot], first_record[slot], "slot {slot}");
        }
    }

    /// A batch the host never receives took table slots that exist only here.
    /// Keeping them would have every later record index one entry past what the
    /// host holds — a neighbour's address read as this message's, silently.
    #[test]
    fn an_abandoned_batch_rolls_the_table() {
        let mut batch = MessageWireBatch::default();
        batch.push(&inbound("SENT", 8)).expect("packs");
        let mut out = Vec::new();
        batch.write_and_reset(&mut out);
        assert_eq!(batch.strings.held, 2, "one address form and the push name");

        // A run cancelled mid-batch: the definitions were never written out.
        batch.push(&inbound("DROPPED", 8)).expect("packs");
        let mut other = MessageInfo::default();
        other.source.chat = "120363@g.us".parse().expect("valid chat jid");
        other.source.sender = "5511:7@s.whatsapp.net".parse().expect("valid sender jid");
        other.id = "DROPPED-2".into();
        other.push_name = "Someone Else".into();
        batch
            .push(
                &InboundMessage::builder()
                    .message(Arc::new(Message::default()))
                    .info(Arc::new(other))
                    .build(),
            )
            .expect("packs");
        assert!(batch.strings.defined() > 0, "the drop has to matter");
        batch.reset();

        assert_eq!(batch.strings.held, 0, "the table kept unsent definitions");
        assert_eq!(
            batch.strings.flags, PACKED_FLAG_RESET_CACHES,
            "the roll was not announced"
        );
        batch.push(&inbound("AFTER", 8)).expect("packs");
        assert_eq!(
            definitions_of(&batch),
            ["5511999@s.whatsapp.net", "Peer"],
            "the next batch has to redefine what the host lost"
        );
        assert_eq!(batch.info_records[0], 0.0, "indices restart at the table's");
    }

    /// An abandoned batch that defined nothing has nothing to take back, so it
    /// must not throw the table away either.
    #[test]
    fn an_empty_abandon_keeps_the_table() {
        let mut batch = MessageWireBatch::default();
        batch.push(&inbound("SENT", 8)).expect("packs");
        let mut out = Vec::new();
        batch.write_and_reset(&mut out);
        let held = batch.strings.held;

        batch.reset();
        assert_eq!(batch.strings.held, held);
        assert_eq!(batch.strings.flags, 0);
        batch.push(&inbound("AFTER", 8)).expect("packs");
        assert!(definitions_of(&batch).is_empty());
    }

    /// The table is bounded, so a long session cannot grow it without limit —
    /// and the roll that bounds it is announced rather than inferred.
    #[test]
    fn the_table_rolls_at_its_entry_ceiling() {
        let mut batch = MessageWireBatch::default();
        let mut out = Vec::new();
        let mut rolled = None;
        for round in 0..PACKED_STRING_CACHE_MAX {
            let mut info = MessageInfo::default();
            info.source.chat = "5511999@s.whatsapp.net".parse().expect("valid chat jid");
            info.source.sender = "5511999:9@s.whatsapp.net"
                .parse()
                .expect("valid sender jid");
            info.id = "CEILING".into();
            // A distinct push name each round is what fills the table.
            info.push_name = format!("Peer {round}");
            batch
                .push(
                    &InboundMessage::builder()
                        .message(Arc::new(Message::default()))
                        .info(Arc::new(info))
                        .build(),
                )
                .expect("packs");
            out.clear();
            batch.write_and_reset(&mut out);
            if batch.strings.flags & PACKED_FLAG_RESET_CACHES != 0 {
                rolled = Some(round);
                break;
            }
        }
        let rolled = rolled.expect("the table never reached its ceiling");
        assert_eq!(batch.strings.held, 0, "a rolled table still holds entries");
        assert!(
            rolled + 1 >= PACKED_STRING_CACHE_MAX - 2,
            "rolled too early"
        );

        batch.push(&inbound("AFTER", 8)).expect("packs");
        assert_eq!(
            definitions_of(&batch),
            ["5511999@s.whatsapp.net", "Peer"],
            "the batch after a roll has to redefine what it references"
        );
    }

    /// Builds a one-message batch carrying `push_name`.
    fn named(id: &str, push_name: String) -> InboundMessage {
        let mut info = MessageInfo::default();
        info.source.chat = "5511999@s.whatsapp.net".parse().expect("valid chat jid");
        info.source.sender = "5511999:9@s.whatsapp.net"
            .parse()
            .expect("valid sender jid");
        info.id = id.into();
        info.push_name = push_name;
        InboundMessage::builder()
            .message(Arc::new(Message::default()))
            .info(Arc::new(info))
            .build()
    }

    /// The table is what keeps a peer-sized push name from being written once
    /// per message. A run of 32 sharing one 1 MiB name is the shape that makes
    /// the difference: one copy on the wire, not thirty-two.
    #[test]
    fn an_oversized_push_name_is_written_once_per_batch() {
        let oversized = "n".repeat(1024 * 1024);
        let mut batch = MessageWireBatch::default();
        for round in 0..32 {
            batch
                .push(&named(&format!("HUGE-{round}"), oversized.clone()))
                .expect("packs");
        }

        assert_eq!(batch.len(), 32);
        assert_eq!(
            definitions_of(&batch),
            ["5511999@s.whatsapp.net", &oversized],
            "the push name was written per message"
        );
        assert!(
            batch.strings.region_len() < oversized.len() * 2,
            "the region carries more than one copy: {} bytes",
            batch.strings.region_len()
        );

        // It also blows the byte ceiling, so the batch that carries it tells
        // the host to drop the table the moment it has been read.
        let mut out = Vec::new();
        batch.write_flat(&mut out);
        let flags = u32::from_le_bytes(out[20..24].try_into().expect("4 bytes"));
        assert_eq!(
            flags & PACKED_FLAG_CLEAR_AFTER,
            PACKED_FLAG_CLEAR_AFTER,
            "the host would have held it until the next batch"
        );
        batch.commit();
        assert_eq!(batch.strings.held, 0, "the writer kept it");
    }

    /// The aggregate ceiling still bounds what the table holds, and rolls the
    /// same announced way the entry ceiling does.
    #[test]
    fn the_table_rolls_at_its_byte_ceiling() {
        const FILLER_BYTES: usize = 240;
        let mut batch = MessageWireBatch::default();
        let mut out = Vec::new();
        // Long enough that the byte ceiling is reached well before the entry one.
        let filler = "n".repeat(FILLER_BYTES);
        let mut rolled = false;
        for round in 0..PACKED_STRING_CACHE_MAX {
            let push_name = format!("{round}{filler}");
            batch.push(&named("BYTES", push_name)).expect("packs");
            out.clear();
            batch.write_and_reset(&mut out);
            if batch.strings.flags & PACKED_FLAG_RESET_CACHES != 0 {
                rolled = true;
                assert!(
                    round < PACKED_STRING_CACHE_MAX - 1,
                    "the entry ceiling rolled first, so this proves nothing"
                );
                break;
            }
        }
        assert!(rolled, "the table never reached its byte ceiling");
        assert_eq!(batch.strings.held, 0, "a rolled table still holds entries");
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
        let definition_count = u32_at(4);
        assert_eq!(definition_count, 3, "chat, sender, pushName");
        let message_bytes = u32_at(8);
        let string_bytes = u32_at(12);
        assert_eq!(u32_at(16), MESSAGE_WIRE_INFO_RECORD_WIDTH, "record width");
        assert_eq!(
            u32_at(20),
            PACKED_FLAG_RESET_CACHES as usize,
            "a fresh writer claims the host's table"
        );

        // Records lead so they land 8-aligned behind the header.
        assert_eq!(MESSAGE_WIRE_HEADER_BYTES % 8, 0);
        let records_end = MESSAGE_WIRE_HEADER_BYTES + MESSAGE_WIRE_INFO_RECORD_WIDTH * 8;
        let chat_slot = f64::from_le_bytes(
            out[MESSAGE_WIRE_HEADER_BYTES..MESSAGE_WIRE_HEADER_BYTES + 8]
                .try_into()
                .expect("8 bytes"),
        );
        assert_eq!(chat_slot, 0.0, "chat is defined first");

        // Then the two offset tables, each carrying its leading sentinel.
        assert_eq!(u32_at(records_end), 0, "payload offset sentinel");
        assert_eq!(u32_at(records_end + 4), message_bytes, "payload end");
        let definition_offsets_at = records_end + 8;
        assert_eq!(
            u32_at(definition_offsets_at),
            0,
            "definition offset sentinel"
        );

        let payloads_at = definition_offsets_at + (definition_count + 1) * 4;
        let strings_at = payloads_at + message_bytes;
        assert_eq!(out.len(), strings_at + string_bytes, "no trailing padding");
        // Definitions, then the inline values in record order.
        let definition_bytes = u32_at(definition_offsets_at + definition_count * 4);
        assert_eq!(
            std::str::from_utf8(&out[strings_at..strings_at + definition_bytes])
                .expect("definition bytes are UTF-8"),
            "120363@g.us5511@s.whatsapp.netAlice"
        );
        assert_eq!(
            std::str::from_utf8(&out[strings_at + definition_bytes..strings_at + string_bytes])
                .expect("inline bytes are UTF-8"),
            "WIRE-1"
        );
    }

    /// The two sides of this format are hand-mirrored, so one fixture is pinned
    /// byte for byte on both. The same literal is asserted against
    /// `encodeMessageWireBatch` in `tests/message-wire-table.test.ts`; a change
    /// to either writer that the other does not follow fails here or there
    /// rather than in a host's decode.
    #[test]
    fn message_wire_batch_matches_the_host_encoder_byte_for_byte() {
        let mut info = MessageInfo::default();
        info.source.chat = "120363@g.us".parse().expect("valid chat jid");
        info.source.sender = "5511:7@s.whatsapp.net".parse().expect("valid sender jid");
        info.id = "WIRE-1".into();
        info.push_name = "Alice".into();
        assert_eq!(
            info.timestamp.timestamp(),
            0,
            "the fixture pins timestamp 0"
        );

        let mut batch = MessageWireBatch::default();
        batch
            .push(
                &InboundMessage::builder()
                    .message(Arc::new(Message::default()))
                    .info(Arc::new(info))
                    .build(),
            )
            .expect("packs");
        let mut out = Vec::new();
        batch.write_flat(&mut out);

        let hex: String = out.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, MESSAGE_WIRE_GOLDEN_HEX);
    }

    /// Shared with `tests/message-wire-table.test.ts`. Regenerate both together.
    ///
    /// header: 1 message, 3 definitions, 0 payload bytes, 41 string bytes,
    /// record width 10, flags = reset. Then the record — chat 0, sender 1,
    /// senderAlt/recipientAlt absent, id 6 bytes inline, pushName 2, timestamp
    /// 0, no flags, no request id, no edit — the two offset tables, and the
    /// string region: the three definitions, then the inline id.
    const MESSAGE_WIRE_GOLDEN_HEX: &str = concat!(
        "010000000300000000000000290000000a00000001000000",
        "0000000000000000",
        "000000000000f03f",
        "0000000000000000",
        "0000000000000000",
        "0000000000001840",
        "0000000000000040",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "000000000b0000001e00000023000000",
        "31323033363340672e7573",
        "3535313140732e77686174736170702e6e6574",
        "416c696365",
        "574952452d31",
    );

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
    /// retained-payload threshold, at it, and above it. The table outlives the
    /// batch, so the reused encoder is cleared to a fresh one first — without
    /// that the two are deliberately different, which the neighbouring tests
    /// are the ones to check.
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
                *encoder = MessageWireBatch::default();
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
            *encoder = MessageWireBatch::default();
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

    /// The host cuts one string region by the lengths carried in the records,
    /// so those lengths have to count the same unit the region is written in:
    /// UTF-8 bytes. A value outside ASCII is where a UTF-16 count would differ,
    /// and every value here is the peer's own text — an unrecognized `type`
    /// attribute and the message ids it acknowledges.
    #[test]
    fn packed_batch_lengths_count_utf8_bytes() {
        let kind = "leído";
        let id = "Ação-1";
        assert_ne!(
            kind.len(),
            kind.encode_utf16().count(),
            "the values have to be non-ASCII for this to test anything"
        );

        let receipt = Event::Receipt(
            wacore::types::events::Receipt::builder()
                .source(wacore::types::message::MessageSource {
                    chat: "5511999@s.whatsapp.net".parse().expect("valid chat jid"),
                    sender: "5511999@s.whatsapp.net".parse().expect("valid sender jid"),
                    ..Default::default()
                })
                .message_ids(vec![id.to_string()])
                .timestamp(Default::default())
                .r#type(wacore::types::presence::ReceiptType::Other(
                    kind.to_string(),
                ))
                .offline(false)
                .build(),
        );

        let mut out = Vec::new();
        ReceiptWireBatch::with_encoder(|encoder| {
            *encoder = ReceiptWireBatch::default();
            encoder.begin();
            encoder.push(&receipt).expect("packs");
            encoder.write_and_reset(&mut out);
        });

        let u32_at =
            |at: usize| u32::from_le_bytes(out[at..at + 4].try_into().expect("4 bytes")) as usize;
        let u16_at =
            |at: usize| u16::from_le_bytes(out[at..at + 2].try_into().expect("2 bytes")) as usize;
        let new_strings = u32_at(4);
        let records_at = PACKED_HEADER_BYTES + new_strings * 4 + u32_at(8) * 11;
        let region_at = out.len() - u32_at(12);

        // Walk the definitions the way the host does: each length in slot order,
        // cutting the region as it goes.
        let mut cursor = region_at;
        let definitions: Vec<&str> = (0..new_strings)
            .map(|i| {
                let length = u16_at(PACKED_HEADER_BYTES + i * 4 + 2);
                let value = std::str::from_utf8(&out[cursor..cursor + length])
                    .expect("a definition ends on a character boundary");
                cursor += length;
                value
            })
            .collect();
        assert_eq!(definitions, ["5511999", "s.whatsapp.net", kind]);

        // Record: u8 flags | 8 x u16 slots | f64 timestamp | u8 id count | u16
        // id length. The inline values follow the definitions in the region.
        let id_length = u16_at(records_at + 1 + 8 * 2 + 8 + 1);
        assert_eq!(id_length, id.len(), "an inline length counts UTF-8 bytes");
        assert_eq!(&out[cursor..cursor + id_length], id.as_bytes());
        assert_eq!(cursor + id_length, out.len(), "the region ends with it");
    }

    /// Dedup must not confuse a value with a longer one that starts the same
    /// way, and a repeat must land on the slot the host already holds.
    #[test]
    fn string_table_distinguishes_prefixes() {
        let mut table = WireStringTable::default();
        let a = table.cache("a");
        let ab = table.cache("ab");
        let a_again = table.cache("a");
        assert_eq!(a, 0);
        assert_eq!(ab, 1);
        assert_eq!(a_again, 0, "an exact repeat still dedups");
        assert_eq!(table.defined(), 2);
        assert_eq!(table.definitions, b"aab");
    }

    /// Push names and message ids come from the wire, so an oversized one must
    /// not pin the reused buffers' peak for the rest of the session.
    #[test]
    fn string_table_capacity_stays_bounded() {
        // Past both ceilings: the buffers must shrink back, and the table must
        // roll rather than hold the value until the entry ceiling is reached.
        let oversized = "n".repeat(WIRE_STRING_CACHE_MAX_BYTES + 1);
        assert!(oversized.len() > WIRE_STRING_TABLE_RETAINED_BYTES);
        let mut table = WireStringTable::default();
        table.cache(&oversized);
        table.write_inline(&oversized);
        table.cache_jid(&"5511999@s.whatsapp.net".parse().expect("valid jid"));
        assert!(table.definitions.capacity() > WIRE_STRING_TABLE_RETAINED_BYTES);
        assert!(table.inline.capacity() > WIRE_STRING_TABLE_RETAINED_BYTES);

        table.commit();
        for (name, capacity) in [
            ("definitions", table.definitions.capacity()),
            ("inline", table.inline.capacity()),
            ("scratch", table.scratch.capacity()),
        ] {
            assert!(
                capacity <= WIRE_STRING_TABLE_RETAINED_BYTES,
                "{name} kept {capacity} bytes"
            );
        }

        // The oversized value also took the table past its byte ceiling, so the
        // commit rolled it: the next batch defines from an empty table again.
        assert_eq!(table.flags, PACKED_FLAG_RESET_CACHES);
        assert_eq!(table.cache("after"), 0);
        assert_eq!(table.defined(), 1);
    }

    /// Reuse stops at the boundary: a batch the host holds must not be
    /// rewritten by the next one.
    #[test]
    fn crossed_batches_do_not_alias() {
        let first = as_bytes(cross_flat_batch(BatchBuffer::Owned, |out| {
            out.extend_from_slice(&[1u8; 64])
        }));
        let _second = cross_flat_batch(BatchBuffer::Owned, |out| out.extend_from_slice(&[2u8; 64]));
        assert_eq!(
            first.to_vec(),
            vec![1u8; 64],
            "the first batch was rewritten"
        );
    }

    /// A batch handed to the host is backed by host memory, not by a view into
    /// the linear memory the encoder writes into: growing WASM memory detaches
    /// the latter, and the host callback re-enters WASM by design.
    #[test]
    fn crossed_batch_survives_linear_memory_growth() {
        let batch = as_bytes(cross_flat_batch(BatchBuffer::Owned, |out| {
            out.extend_from_slice(&[7u8; 128])
        }));

        // Grow linear memory while the host still holds the batch.
        assert_ne!(
            core::arch::wasm32::memory_grow::<0>(16),
            usize::MAX,
            "memory.grow should succeed"
        );

        assert_eq!(batch.length(), 128, "the batch was detached by the growth");
        assert_eq!(batch.to_vec(), vec![7u8; 128]);
    }

    /// The scratch is reused, so one oversized batch must not pin its peak for
    /// the rest of the session.
    #[test]
    fn crossing_scratch_capacity_stays_bounded() {
        let big = MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES * 2;
        let oversized = as_bytes(cross_flat_batch(BatchBuffer::Owned, |out| {
            out.resize(big, 9)
        }));
        assert_eq!(oversized.length() as usize, big);

        // The next batch reveals what the scratch kept: it has to grow again.
        let mut capacity_after = 0;
        let next = as_bytes(cross_flat_batch(BatchBuffer::Owned, |out| {
            capacity_after = out.capacity();
            out.extend_from_slice(&[3u8; 32]);
        }));
        assert!(
            capacity_after <= MESSAGE_WIRE_RETAINED_PAYLOAD_BYTES,
            "scratch kept {capacity_after} bytes"
        );
        assert_eq!(next.to_vec(), vec![3u8; 32]);
        assert_eq!(
            oversized.get_index(0),
            9,
            "the oversized batch was rewritten"
        );
    }

    /// Two typed arrays over the same bytes.
    fn same_buffer(a: &js_sys::Uint8Array, b: &js_sys::Uint8Array) -> bool {
        js_sys::Object::is(&a.buffer().into(), &b.buffer().into())
    }

    /// The opt-in has to actually reuse the buffer, or it buys nothing. The
    /// second half of this is what the contract forbids: a window kept past its
    /// callback finds the next batch's bytes.
    #[test]
    fn borrowed_batches_share_one_host_buffer() {
        reset_borrowed_batches();
        let first = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[1u8; 48])
        }));
        assert_eq!(first.to_vec(), vec![1u8; 48]);

        let second = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[2u8; 64])
        }));
        assert_eq!(second.to_vec(), vec![2u8; 64]);
        assert!(
            same_buffer(&first, &second),
            "the batches did not share a buffer"
        );
        assert_eq!(first.to_vec(), vec![2u8; 48]);
    }

    /// The shared buffer is host-allocated, so it must survive the `memory.grow`
    /// that a host callback re-entering WASM can trigger.
    #[test]
    fn a_borrowed_batch_survives_linear_memory_growth() {
        reset_borrowed_batches();
        let batch = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[7u8; 128])
        }));

        assert_ne!(
            core::arch::wasm32::memory_grow::<0>(16),
            usize::MAX,
            "memory.grow should succeed"
        );

        assert_eq!(batch.length(), 128, "the batch was detached by the growth");
        assert_eq!(batch.to_vec(), vec![7u8; 128]);
    }

    /// An outlier neither grows the shared buffer nor loses its tail to it.
    #[test]
    fn an_oversized_borrowed_batch_gets_its_own_buffer() {
        reset_borrowed_batches();
        let held = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[5u8; 32])
        }));
        let big = BORROWED_BATCH_BUFFER_BYTES + 1;
        let oversized = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.resize(big, 6)
        }));
        assert_eq!(oversized.length() as usize, big);
        assert_eq!(oversized.to_vec(), vec![6u8; big]);
        assert!(
            !same_buffer(&held, &oversized),
            "the outlier landed in the shared buffer"
        );
        assert_eq!(
            held.to_vec(),
            vec![5u8; 32],
            "the shared buffer was rewritten"
        );

        let next = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[3u8; 16])
        }));
        assert!(same_buffer(&held, &next), "the shared buffer was replaced");
        assert_eq!(
            next.buffer().byte_length() as usize,
            BORROWED_BATCH_BUFFER_BYTES,
            "the outlier pinned a larger shared buffer"
        );
    }

    /// The guard here is the last line of defense, below whatever the delivery
    /// channel decides, so it gets its own coverage.
    #[test]
    fn a_revoked_borrow_falls_back_to_its_own_buffer() {
        reset_borrowed_batches();
        let held = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[5u8; 32])
        }));

        revoke_borrowed_batches();
        let after = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[6u8; 32])
        }));
        assert!(
            !same_buffer(&held, &after),
            "a revoked borrow reused the shared buffer"
        );
        assert_eq!(
            held.to_vec(),
            vec![5u8; 32],
            "the shared buffer was rewritten"
        );

        // Revoking also drops the buffer, so the window stays unreachable even
        // if something later decides to borrow again.
        reset_borrowed_batches();
        let next = as_bytes(cross_flat_batch(BatchBuffer::Borrowed, |out| {
            out.extend_from_slice(&[7u8; 32])
        }));
        assert!(!same_buffer(&held, &next), "the abandoned buffer came back");
        assert_eq!(held.to_vec(), vec![5u8; 32]);
    }

    /// Opting in changes where the bytes land, never what they are.
    #[test]
    fn a_borrowed_receipt_batch_carries_the_bytes_an_owned_one_would() {
        let receipt = || {
            Event::Receipt(
                wacore::types::events::Receipt::builder()
                    .source(wacore::types::message::MessageSource {
                        chat: "5511999@s.whatsapp.net".parse().expect("valid chat jid"),
                        sender: "5511999:9@s.whatsapp.net"
                            .parse()
                            .expect("valid sender jid"),
                        ..Default::default()
                    })
                    .message_ids(vec!["RCPT-1".into()])
                    .timestamp(Default::default())
                    .r#type(wacore::types::presence::ReceiptType::Delivered)
                    .offline(false)
                    .build(),
            )
        };

        reset_borrowed_batches();
        // The encoder's caches persist across batches, so both arms have to
        // start from the same cache state to be comparable.
        let encode = |buffer| {
            ReceiptWireBatch::with_encoder(|encoder| {
                *encoder = ReceiptWireBatch::default();
                encoder.begin();
                encoder.push(&receipt()).expect("packs");
                as_bytes(encoder.finish(buffer).expect("crosses")).to_vec()
            })
        };
        assert_eq!(encode(BatchBuffer::Borrowed), encode(BatchBuffer::Owned));
    }

    /// The dispatch future can be dropped at any suspension point, and it takes
    /// a buffered envelope with it. Those segments were written by encoders that
    /// already counted their definitions as held, so the tables have to roll —
    /// otherwise the next batch's slots index past what the host has.
    #[test]
    fn a_dropped_envelope_rolls_every_table() {
        MessageWireBatch::with_encoder(|encoder| *encoder = MessageWireBatch::default());
        ReceiptWireBatch::with_encoder(|encoder| *encoder = ReceiptWireBatch::default());

        // A delivered batch: the host holds what it defined.
        let mut out = Vec::new();
        MessageWireBatch::with_encoder(|encoder| {
            encoder.push(&inbound("DELIVERED", 8)).expect("packs");
            encoder.write_and_reset(&mut out);
            assert_eq!(encoder.strings.held, 2);
            assert_eq!(encoder.strings.flags, 0);
        });

        // A run buffered into an envelope that is then dropped unflushed.
        {
            let mut envelope = EventWireEnvelope::default();
            MessageWireBatch::with_encoder(|encoder| {
                encoder.push(&inbound("LOST", 8)).expect("packs");
                let records = encoder.len();
                envelope.push_segment(EVENT_SEGMENT_KIND_MESSAGE, records, |out| {
                    encoder.write_and_reset(out)
                });
            });
            assert!(!envelope.is_empty());
        }

        MessageWireBatch::with_encoder(|encoder| {
            assert_eq!(encoder.strings.held, 0, "the lost segment stayed held");
            assert_eq!(encoder.strings.flags, PACKED_FLAG_RESET_CACHES);
            encoder.push(&inbound("AFTER", 8)).expect("packs");
            assert_eq!(
                definitions_of(encoder),
                ["5511999@s.whatsapp.net", "Peer"],
                "the batch after the loss has to redefine what it references"
            );
            encoder.reset();
        });
        ReceiptWireBatch::with_encoder(|encoder| {
            assert!(encoder.writer.string_cache.is_empty());
            assert_eq!(
                encoder.writer.flags & PACKED_FLAG_RESET_CACHES,
                PACKED_FLAG_RESET_CACHES
            );
        });
    }

    /// A host that defers its decode reads batches in an order the writer
    /// cannot see, and a table spanning batches is only sound in delivery
    /// order. Giving it up makes every batch self-contained, which reads the
    /// same whenever it is decoded — the optimization goes, not the values.
    #[test]
    fn revoking_the_tables_makes_every_batch_self_contained() {
        reset_packed_tables();
        MessageWireBatch::with_encoder(|encoder| *encoder = MessageWireBatch::default());

        let mut out = Vec::new();
        MessageWireBatch::with_encoder(|encoder| {
            encoder.push(&inbound("BEFORE-1", 8)).expect("packs");
            encoder.write_and_reset(&mut out);
            encoder.push(&inbound("BEFORE-2", 8)).expect("packs");
            assert!(
                definitions_of(encoder).is_empty(),
                "the table is supposed to span batches until it is revoked"
            );
            encoder.write_and_reset(&mut out);
        });

        revoke_packed_tables();

        // Two batches in a row, each defining everything it names and each
        // asking the reader to clear: decoded in either order, both read right.
        for round in 0..2 {
            MessageWireBatch::with_encoder(|encoder| {
                encoder.push(&inbound("AFTER", 8)).expect("packs");
                assert_eq!(
                    definitions_of(encoder),
                    ["5511999@s.whatsapp.net", "Peer"],
                    "round {round} leaned on a table the reader may not have"
                );
                assert_eq!(encoder.strings.flags, PACKED_FLAG_RESET_CACHES);
                assert_eq!(encoder.info_records[0], 0.0, "slots restart every batch");
                out.clear();
                encoder.write_and_reset(&mut out);
            });
        }

        // The packed writers give theirs up too, under the same flag.
        ReceiptWireBatch::with_encoder(|encoder| {
            assert!(encoder.writer.string_cache.is_empty());
            assert_eq!(
                encoder.writer.flags & PACKED_FLAG_RESET_CACHES,
                PACKED_FLAG_RESET_CACHES
            );
        });
        reset_packed_tables();
        MessageWireBatch::with_encoder(|encoder| *encoder = MessageWireBatch::default());
    }

    /// Crossing the envelope is not losing it, so the tables must survive.
    #[test]
    fn a_crossed_envelope_keeps_every_table() {
        MessageWireBatch::with_encoder(|encoder| *encoder = MessageWireBatch::default());
        let mut envelope = EventWireEnvelope::default();
        MessageWireBatch::with_encoder(|encoder| {
            encoder.push(&inbound("SENT", 8)).expect("packs");
            let records = encoder.len();
            envelope.push_segment(EVENT_SEGMENT_KIND_MESSAGE, records, |out| {
                encoder.write_and_reset(out)
            });
        });
        let _crossed = envelope.finish(BatchBuffer::Owned);
        drop(envelope);

        MessageWireBatch::with_encoder(|encoder| {
            assert_eq!(encoder.strings.held, 2, "a delivered run rolled the table");
            assert_eq!(encoder.strings.flags, 0);
            encoder.reset();
        });
    }

    /// The encoder is process-wide, so the borrow must end before any host
    /// callback runs. Two batches built back to back must stay independent.
    #[test]
    fn consecutive_batches_are_serialized_and_independent() {
        let first = MessageWireBatch::with_encoder(|encoder| {
            *encoder = MessageWireBatch::default();
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
