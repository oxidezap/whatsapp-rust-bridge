/**
 * Packed wire-batch codecs (message metadata, receipts, server acks).
 *
 * The bridge crosses hot per-event metadata as fixed-width numeric records
 * plus a per-batch deduplicated string table: repeated addresses, push names
 * and ack classes pay one decode per batch instead of one FFI object build
 * per event. This module is the single host-side owner of every record
 * layout; the Rust writers live in `src/wasm_client.rs`.
 */

const utf8Decoder = new TextDecoder();
const utf8Encoder = new TextEncoder();

// ---------------------------------------------------------------------------
// Shared string-table helpers
// ---------------------------------------------------------------------------

function decodeStringTable(data: Uint8Array, offsets: Uint32Array): string[] {
  const table: string[] = new Array(Math.max(0, offsets.length - 1));
  for (let i = 0; i + 1 < offsets.length; i++) {
    table[i] = utf8Decoder.decode(data.subarray(offsets[i]!, offsets[i + 1]!));
  }
  return table;
}

function tableAccessors(table: string[]) {
  const required = (index: number): string => {
    const value = table[index];
    if (value === undefined) throw new RangeError("packed record references a missing table entry");
    return value;
  };
  // Optional slots carry table index + 1 so 0 can mean absent.
  const optional = (raw: number): string | undefined => (raw === 0 ? undefined : required(raw - 1));
  return { required, optional };
}

/** Build-side mirror of the Rust `WireStringTable`, for tests and tooling. */
class StringTableBuilder {
  private readonly table: string[] = [];
  private readonly indexByValue = new Map<string, number>();

  intern(value: string): number {
    const existing = this.indexByValue.get(value);
    if (existing !== undefined) return existing;
    const index = this.table.length;
    this.table.push(value);
    this.indexByValue.set(value, index);
    return index;
  }

  internOptional(value: string | undefined): number {
    return value === undefined ? 0 : this.intern(value) + 1;
  }

  pack(): { data: Uint8Array; offsets: Uint32Array } {
    const encoder = new TextEncoder();
    const encoded = this.table.map(value => encoder.encode(value));
    const offsets = new Uint32Array(encoded.length + 1);
    let total = 0;
    for (let i = 0; i < encoded.length; i++) {
      total += encoded[i]!.length;
      offsets[i + 1] = total;
    }
    const data = new Uint8Array(total);
    for (let i = 0; i < encoded.length; i++) data.set(encoded[i]!, offsets[i]!);
    return { data, offsets };
  }
}

// ---------------------------------------------------------------------------
// JID components
// ---------------------------------------------------------------------------

/** Serde shape of the core's `Jid` as carried by bridge events. */
export interface WireJid {
  user: string;
  server: string;
  agent: number;
  device: number;
  integrator: number;
}

/** Slots per deduplicated JID entry in `jidComponents`. */
const JID_COMPONENT_WIDTH = 5;

/** Materialize the per-batch JID table from its packed components. */
function decodeJidTable(components: Float64Array, table: string[]): WireJid[] {
  if (components.length % JID_COMPONENT_WIDTH !== 0) {
    throw new RangeError("jid components length is not a whole number of entries");
  }
  const count = components.length / JID_COMPONENT_WIDTH;
  const jids: WireJid[] = new Array(count);
  for (let i = 0; i < count; i++) {
    const base = i * JID_COMPONENT_WIDTH;
    const user = table[components[base]!];
    const server = table[components[base + 1]!];
    if (user === undefined || server === undefined) {
      throw new RangeError("jid components reference a missing table entry");
    }
    jids[i] = {
      user,
      server,
      agent: components[base + 2]!,
      device: components[base + 3]!,
      integrator: components[base + 4]!,
    };
  }
  return jids;
}

// ---------------------------------------------------------------------------
// Message metadata records
// ---------------------------------------------------------------------------

/** Decoded per-message metadata envelope. */
export interface MessageWireInfo {
  /** Canonical device-independent chat address. */
  chat: string;
  /** Canonical device-independent author address. */
  sender: string;
  senderAlt?: string;
  recipientAlt?: string;
  isFromMe: boolean;
  isGroup: boolean;
  id: string;
  /** Unix timestamp in seconds. */
  timestamp: number;
  pushName: string;
  isViewOnce: boolean;
  isOffline: boolean;
  unavailableRequestId?: string;
  /** Raw protocol edit attribute; absent when the attribute was empty. */
  edit?: string;
}

export const MESSAGE_WIRE_INFO_RECORD_WIDTH = 10;
const INFO_SLOT_CHAT = 0;
const INFO_SLOT_SENDER = 1;
const INFO_SLOT_SENDER_ALT = 2;
const INFO_SLOT_RECIPIENT_ALT = 3;
const INFO_SLOT_ID = 4;
const INFO_SLOT_PUSH_NAME = 5;
const INFO_SLOT_TIMESTAMP = 6;
const INFO_SLOT_FLAGS = 7;
const INFO_SLOT_UNAVAILABLE_REQUEST_ID = 8;
const INFO_SLOT_EDIT = 9;
const INFO_FLAG_FROM_ME = 1 << 0;
const INFO_FLAG_GROUP = 1 << 1;
const INFO_FLAG_VIEW_ONCE = 1 << 2;
const INFO_FLAG_OFFLINE = 1 << 3;

/**
 * Byte layout of a message batch, written by `MessageWireBatch::write_flat` in
 * `src/wire_batch.rs`:
 *
 * ```text
 * header (24 B): u32 messages | u32 strings | u32 message_bytes
 *                u32 string_bytes | u32 record_width | u32 reserved
 * records:       messages * record_width * f64
 * offsets:       (messages + 1) * u32
 * str offsets:   (strings + 1) * u32
 * payloads:      message_bytes
 * strings:       string_bytes
 * ```
 *
 * The f64 block leads so it lands 8-aligned behind the 24-byte header, and the
 * u32 tables follow at 4-aligned offsets. That is what lets this decoder build
 * views straight over the one buffer the bridge crossed, instead of the five
 * typed arrays an object envelope would have carried.
 */
const MESSAGE_WIRE_HEADER_BYTES = 24;
const HEADER_SLOT_MESSAGES = 0;
const HEADER_SLOT_STRINGS = 4;
const HEADER_SLOT_MESSAGE_BYTES = 8;
const HEADER_SLOT_STRING_BYTES = 12;
const HEADER_SLOT_RECORD_WIDTH = 16;

/** Decoded message batch: payload slices plus one envelope per message. */
export interface MessageWireBatchView {
  /** Concatenated `proto.Message` payloads, in event order. */
  messageData: Uint8Array;
  /** N + 1 byte offsets delimiting the N payloads in `messageData`. */
  messageOffsets: Uint32Array;
  /** One decoded envelope per message, parallel to the payload offsets. */
  infos: MessageWireInfo[];
}

/**
 * Typed-array views need their byte offset aligned to the element size. The
 * bridge always crosses a freshly allocated `Uint8Array` (byteOffset 0), so the
 * layout is aligned by construction; a caller-built batch over a subarray may
 * not be, and is copied once rather than rejected.
 */
function alignedBuffer(buffer: Uint8Array): Uint8Array {
  return (buffer.byteOffset + MESSAGE_WIRE_HEADER_BYTES) % 8 === 0 ? buffer : new Uint8Array(buffer);
}

/** Decode a packed message batch into payload slices and metadata envelopes. */
export function decodeMessageWireBatch(batch: PackedWireBatch): MessageWireBatchView {
  const buffer = alignedBuffer(batch);
  if (buffer.byteLength < MESSAGE_WIRE_HEADER_BYTES) {
    throw new RangeError("message wire batch is shorter than its header");
  }
  const header = new DataView(buffer.buffer, buffer.byteOffset, MESSAGE_WIRE_HEADER_BYTES);
  const messageCount = header.getUint32(HEADER_SLOT_MESSAGES, true);
  const stringCount = header.getUint32(HEADER_SLOT_STRINGS, true);
  const messageBytes = header.getUint32(HEADER_SLOT_MESSAGE_BYTES, true);
  const stringBytes = header.getUint32(HEADER_SLOT_STRING_BYTES, true);
  const recordWidth = header.getUint32(HEADER_SLOT_RECORD_WIDTH, true);
  if (recordWidth !== MESSAGE_WIRE_INFO_RECORD_WIDTH) {
    throw new RangeError(
      `message wire batch record width ${recordWidth} does not match the expected ${MESSAGE_WIRE_INFO_RECORD_WIDTH}`,
    );
  }

  const recordBytes = messageCount * recordWidth * 8;
  const offsetBytes = (messageCount + 1) * 4;
  const stringOffsetBytes = (stringCount + 1) * 4;
  const expected =
    MESSAGE_WIRE_HEADER_BYTES + recordBytes + offsetBytes + stringOffsetBytes + messageBytes + stringBytes;
  if (buffer.byteLength < expected) {
    throw new RangeError(`message wire batch is truncated: ${buffer.byteLength} bytes, expected ${expected}`);
  }

  let cursor = MESSAGE_WIRE_HEADER_BYTES;
  const records = new Float64Array(buffer.buffer, buffer.byteOffset + cursor, messageCount * recordWidth);
  cursor += recordBytes;
  const messageOffsets = new Uint32Array(buffer.buffer, buffer.byteOffset + cursor, messageCount + 1);
  cursor += offsetBytes;
  const stringOffsets = new Uint32Array(buffer.buffer, buffer.byteOffset + cursor, stringCount + 1);
  cursor += stringOffsetBytes;
  const messageData = buffer.subarray(cursor, cursor + messageBytes);
  cursor += messageBytes;
  const stringData = buffer.subarray(cursor, cursor + stringBytes);

  const { required, optional } = tableAccessors(decodeStringTable(stringData, stringOffsets));
  const infos: MessageWireInfo[] = new Array(messageCount);
  for (let i = 0; i < messageCount; i++) {
    const base = i * recordWidth;
    const flags = records[base + INFO_SLOT_FLAGS]!;
    infos[i] = {
      chat: required(records[base + INFO_SLOT_CHAT]!),
      sender: required(records[base + INFO_SLOT_SENDER]!),
      senderAlt: optional(records[base + INFO_SLOT_SENDER_ALT]!),
      recipientAlt: optional(records[base + INFO_SLOT_RECIPIENT_ALT]!),
      isFromMe: (flags & INFO_FLAG_FROM_ME) !== 0,
      isGroup: (flags & INFO_FLAG_GROUP) !== 0,
      id: required(records[base + INFO_SLOT_ID]!),
      timestamp: records[base + INFO_SLOT_TIMESTAMP]!,
      pushName: required(records[base + INFO_SLOT_PUSH_NAME]!),
      isViewOnce: (flags & INFO_FLAG_VIEW_ONCE) !== 0,
      isOffline: (flags & INFO_FLAG_OFFLINE) !== 0,
      unavailableRequestId: optional(records[base + INFO_SLOT_UNAVAILABLE_REQUEST_ID]!),
      edit: optional(records[base + INFO_SLOT_EDIT]!),
    };
  }
  return { messageData, messageOffsets, infos };
}

/** One message as a caller supplies it to `encodeMessageWireBatch`. */
export interface MessageWireEntry {
  /** Encoded `proto.Message` payload. */
  payload: Uint8Array;
  info: MessageWireInfo;
}

/**
 * Inverse of `decodeMessageWireBatch`, for hosts that need to fabricate batches
 * (tests, replay tooling). Mirrors the Rust writer byte for byte.
 */
export function encodeMessageWireBatch(entries: readonly MessageWireEntry[]): PackedWireBatch {
  const strings = new StringTableBuilder();
  const records = new Float64Array(entries.length * MESSAGE_WIRE_INFO_RECORD_WIDTH);
  const messageOffsets = new Uint32Array(entries.length + 1);
  let payloadBytes = 0;
  for (let i = 0; i < entries.length; i++) {
    const { payload, info } = entries[i]!;
    payloadBytes += payload.byteLength;
    messageOffsets[i + 1] = payloadBytes;
    const base = i * MESSAGE_WIRE_INFO_RECORD_WIDTH;
    records[base + INFO_SLOT_CHAT] = strings.intern(info.chat);
    records[base + INFO_SLOT_SENDER] = strings.intern(info.sender);
    records[base + INFO_SLOT_SENDER_ALT] = strings.internOptional(info.senderAlt);
    records[base + INFO_SLOT_RECIPIENT_ALT] = strings.internOptional(info.recipientAlt);
    records[base + INFO_SLOT_ID] = strings.intern(info.id);
    records[base + INFO_SLOT_PUSH_NAME] = strings.intern(info.pushName);
    records[base + INFO_SLOT_TIMESTAMP] = info.timestamp;
    records[base + INFO_SLOT_FLAGS] =
      (info.isFromMe ? INFO_FLAG_FROM_ME : 0) |
      (info.isGroup ? INFO_FLAG_GROUP : 0) |
      (info.isViewOnce ? INFO_FLAG_VIEW_ONCE : 0) |
      (info.isOffline ? INFO_FLAG_OFFLINE : 0);
    records[base + INFO_SLOT_UNAVAILABLE_REQUEST_ID] = strings.internOptional(info.unavailableRequestId);
    records[base + INFO_SLOT_EDIT] = strings.internOptional(info.edit);
  }
  const { data: stringData, offsets: stringOffsets } = strings.pack();

  const total =
    MESSAGE_WIRE_HEADER_BYTES +
    records.byteLength +
    messageOffsets.byteLength +
    stringOffsets.byteLength +
    payloadBytes +
    stringData.byteLength;
  const buffer = new Uint8Array(total);
  const header = new DataView(buffer.buffer, 0, MESSAGE_WIRE_HEADER_BYTES);
  header.setUint32(HEADER_SLOT_MESSAGES, entries.length, true);
  header.setUint32(HEADER_SLOT_STRINGS, Math.max(0, stringOffsets.length - 1), true);
  header.setUint32(HEADER_SLOT_MESSAGE_BYTES, payloadBytes, true);
  header.setUint32(HEADER_SLOT_STRING_BYTES, stringData.byteLength, true);
  header.setUint32(HEADER_SLOT_RECORD_WIDTH, MESSAGE_WIRE_INFO_RECORD_WIDTH, true);

  let cursor = MESSAGE_WIRE_HEADER_BYTES;
  buffer.set(new Uint8Array(records.buffer, records.byteOffset, records.byteLength), cursor);
  cursor += records.byteLength;
  buffer.set(new Uint8Array(messageOffsets.buffer, messageOffsets.byteOffset, messageOffsets.byteLength), cursor);
  cursor += messageOffsets.byteLength;
  buffer.set(new Uint8Array(stringOffsets.buffer, stringOffsets.byteOffset, stringOffsets.byteLength), cursor);
  cursor += stringOffsets.byteLength;
  for (const { payload } of entries) {
    buffer.set(payload, cursor);
    cursor += payload.byteLength;
  }
  buffer.set(stringData, cursor);
  return buffer;
}

// ---------------------------------------------------------------------------
// Receipt and server-ack records
// ---------------------------------------------------------------------------

/**
 * A packed batch IS its buffer: one flat run of header, cache definitions,
 * fixed-width records and string bytes. See the Rust writers in
 * `src/wire_batch.rs`.
 *
 * Crossing the bare typed array rather than a `{ buffer }` object saves an
 * object construction and a property write per batch, and a single message
 * produces three batches (its own, its receipt, its ack).
 */
export type PackedWireBatch = Uint8Array;

/** Serde shape of the core's `Receipt` payload, as the single-event path emits it. */
export interface ReceiptWireData {
  source: {
    chat: WireJid;
    sender: WireJid;
    is_from_me: boolean;
    is_group: boolean;
    addressing_mode?: string;
    sender_alt?: WireJid;
    recipient_alt?: WireJid;
    broadcast_list_owner?: WireJid;
    recipient?: WireJid;
  };
  message_ids: string[];
  /** Unix timestamp in seconds. */
  timestamp: number;
  /** Variant name, or `{ Other: value }` for unrecognized wire values. */
  type: string | { Other: string };
  offline: boolean;
}

/** Serde shape of the core's `ServerAck` payload. */
export interface ServerAckWireData {
  id: string;
  class?: string;
  from?: WireJid;
  /** Unix timestamp in seconds. */
  timestamp?: number;
  error?: string;
}

const PACKED_HEADER_BYTES = 20;
const PACKED_FLAG_RESET_CACHES = 1;
/** Below this many bytes an ASCII region decodes faster in JS than through
 * `TextDecoder`, whose constant cost dominates for short inputs. Reserved for
 * ASCII: the byte-at-a-time read is latin1, which is the same thing only there. */
const TINY_STRING_LIMIT = 100;

const RECEIPT_FLAG_FROM_ME = 1 << 0;
const RECEIPT_FLAG_GROUP = 1 << 1;
const RECEIPT_FLAG_OFFLINE = 1 << 2;
const RECEIPT_FLAG_TYPE_OTHER = 1 << 3;

/**
 * Host side of the packed transport: keeps the string/JID caches that mirror
 * the encoder's, so a repeated address is materialized once per process.
 *
 * Every length the encoder writes counts UTF-8 bytes, so the region is cut in
 * bytes here too. Message ids, ack classes and an `Other` receipt type are the
 * peer's own text, so a region outside ASCII is ordinary, not exceptional.
 *
 * Cached JIDs are frozen and shared across events by design: the encoder only
 * caches values it treats as immutable, and freezing makes that contract
 * enforceable instead of documented.
 */
class PackedBatchReader {
  private readonly strings: string[] = [];
  private readonly jids: WireJid[] = [];
  private view!: DataView;
  private bytes!: Uint8Array;
  private cursor = 0;
  /** Byte offset of the next unread value inside the string region. */
  private inlineCursor = 0;
  /** One decode of the whole string region, cut per value by `take`. */
  private region = "";
  /** UTF-16 index into `region` matching `inlineCursor`; equal while ASCII. */
  private regionUnits = 0;
  private regionIsAscii = true;

  /** Read the header, apply cache definitions and position at the records. */
  begin(buffer: Uint8Array): number {
    this.bytes = buffer;
    this.view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
    if (buffer.byteLength < PACKED_HEADER_BYTES) {
      throw new RangeError("packed batch is shorter than its header");
    }
    const recordCount = this.view.getUint32(0, true);
    const newStringCount = this.view.getUint32(4, true);
    const newJidCount = this.view.getUint32(8, true);
    const stringBytes = this.view.getUint32(12, true);
    const flags = this.view.getUint32(16, true);
    if ((flags & PACKED_FLAG_RESET_CACHES) !== 0) {
      this.strings.length = 0;
      this.jids.length = 0;
    }

    let at = PACKED_HEADER_BYTES;
    const newStringsAt = at;
    at += newStringCount * 4;
    const newJidsAt = at;
    at += newJidCount * 11;
    const recordsAt = at;
    const stringsAt = buffer.byteLength - stringBytes;
    if (stringsAt < recordsAt) throw new RangeError("packed batch regions overlap");

    this.readRegion(stringsAt, stringBytes);

    for (let i = 0; i < newStringCount; i++) {
      const at = newStringsAt + i * 4;
      const slot = this.view.getUint16(at, true);
      this.strings[slot] = this.take(this.view.getUint16(at + 2, true));
    }
    for (let i = 0; i < newJidCount; i++) {
      const at = newJidsAt + i * 11;
      const slot = this.view.getUint16(at, true);
      const user = this.strings[this.view.getUint16(at + 2, true)];
      const server = this.strings[this.view.getUint16(at + 4, true)];
      if (user === undefined || server === undefined) {
        throw new RangeError("packed jid references a missing string slot");
      }
      this.jids[slot] = Object.freeze({
        user,
        server,
        agent: this.view.getUint8(at + 6),
        device: this.view.getUint16(at + 7, true),
        integrator: this.view.getUint16(at + 9, true),
      });
    }

    this.cursor = recordsAt;
    return recordCount;
  }

  /**
   * Take the next `length` bytes of the string region as one value.
   *
   * The walk is affordable because values are taken in the order the region was
   * written, and exact because that region is valid UTF-8 — every value the
   * writer packs is a Rust `&str`.
   */
  private take(length: number): string {
    const start = this.inlineCursor;
    this.inlineCursor += length;
    if (this.regionIsAscii) return this.region.substring(start, this.inlineCursor);

    const region = this.region;
    const from = this.regionUnits;
    let at = from;
    let consumed = 0;
    while (consumed < length && at < region.length) {
      const unit = region.charCodeAt(at);
      if (unit < 0x80) {
        consumed += 1;
        at += 1;
      } else if (unit < 0x800) {
        consumed += 2;
        at += 1;
      } else if (unit >= 0xd800 && unit < 0xdc00) {
        // A high surrogate is always paired here: `TextDecoder` substitutes
        // U+FFFD for anything that has no code point of its own.
        consumed += 4;
        at += 2;
      } else {
        consumed += 3;
        at += 1;
      }
    }
    this.regionUnits = at;
    return region.substring(from, at);
  }

  /**
   * Decode the batch's string region once, and record whether it is ASCII —
   * the one case where a byte length off the wire also indexes the decoded
   * string, and so the case `take` can cut with nothing but `substring`.
   */
  private readRegion(at: number, length: number): void {
    this.inlineCursor = 0;
    this.regionUnits = 0;
    this.region = "";
    this.regionIsAscii = true;
    if (length === 0) return;

    if (length >= TINY_STRING_LIMIT) {
      this.region = utf8Decoder.decode(this.bytes.subarray(at, at + length));
      // Every code point outside ASCII spends more UTF-8 bytes than UTF-16
      // units, so equal counts is exactly an ASCII region — no second pass.
      this.regionIsAscii = this.region.length === length;
      return;
    }

    // Short region: read four bytes at a time and skip both the decoder's
    // constant cost and the subarray allocation. Those reads are latin1, which
    // is UTF-8 only while no byte has its high bit set — so the first one that
    // does abandons the attempt for the decoder, rather than finishing a string
    // that would have to be thrown away.
    let out = "";
    let p = at;
    const wordEnd = at + ((length / 4) | 0) * 4;
    while (p < wordEnd) {
      const word = this.view.getUint32(p, true);
      if ((word & 0x80808080) !== 0) break;
      out += String.fromCharCode(word & 255, (word >> 8) & 255, (word >> 16) & 255, word >>> 24);
      p += 4;
    }
    if (p === wordEnd) {
      while (p < at + length) {
        const byte = this.view.getUint8(p);
        if (byte >= 0x80) break;
        out += String.fromCharCode(byte);
        p++;
      }
    }
    if (p === at + length) {
      this.region = out;
      return;
    }
    this.regionIsAscii = false;
    this.region = utf8Decoder.decode(this.bytes.subarray(at, at + length));
  }

  u8(): number {
    return this.view.getUint8(this.cursor++);
  }

  f64(): number {
    const value = this.view.getFloat64(this.cursor, true);
    this.cursor += 8;
    return value;
  }

  /** Next inline value: its byte length lives in the record, its bytes in the region. */
  inline(): string {
    const length = this.view.getUint16(this.cursor, true);
    this.cursor += 2;
    return this.take(length);
  }

  private slot(): number {
    const slot = this.view.getUint16(this.cursor, true);
    this.cursor += 2;
    return slot;
  }

  str(): string {
    const value = this.strings[this.slot()];
    if (value === undefined) throw new RangeError("packed record references a missing string slot");
    return value;
  }

  strOptional(): string | undefined {
    const raw = this.slot();
    if (raw === 0) return undefined;
    const value = this.strings[raw - 1];
    if (value === undefined) throw new RangeError("packed record references a missing string slot");
    return value;
  }

  jid(): WireJid {
    const value = this.jids[this.slot()];
    if (value === undefined) throw new RangeError("packed record references a missing jid slot");
    return value;
  }

  jidOptional(): WireJid | undefined {
    const raw = this.slot();
    if (raw === 0) return undefined;
    const value = this.jids[raw - 1];
    if (value === undefined) throw new RangeError("packed record references a missing jid slot");
    return value;
  }
}

const receiptReader = new PackedBatchReader();
const serverAckReader = new PackedBatchReader();

export function decodeReceiptWireBatch(batch: PackedWireBatch): ReceiptWireData[] {
  const count = receiptReader.begin(batch);
  const receipts: ReceiptWireData[] = new Array(count);
  for (let i = 0; i < count; i++) {
    const flags = receiptReader.u8();
    const chat = receiptReader.jid();
    const sender = receiptReader.jid();
    const senderAlt = receiptReader.jidOptional();
    const recipientAlt = receiptReader.jidOptional();
    const broadcastListOwner = receiptReader.jidOptional();
    const recipient = receiptReader.jidOptional();
    const addressingMode = receiptReader.strOptional();
    const type = receiptReader.str();
    const timestamp = receiptReader.f64();
    const idCount = receiptReader.u8();
    const messageIds: string[] = new Array(idCount);
    for (let j = 0; j < idCount; j++) messageIds[j] = receiptReader.inline();
    receipts[i] = {
      source: {
        chat,
        sender,
        is_from_me: (flags & RECEIPT_FLAG_FROM_ME) !== 0,
        is_group: (flags & RECEIPT_FLAG_GROUP) !== 0,
        addressing_mode: addressingMode,
        sender_alt: senderAlt,
        recipient_alt: recipientAlt,
        broadcast_list_owner: broadcastListOwner,
        recipient,
      },
      message_ids: messageIds,
      timestamp,
      type: (flags & RECEIPT_FLAG_TYPE_OTHER) !== 0 ? { Other: type } : type,
      offline: (flags & RECEIPT_FLAG_OFFLINE) !== 0,
    };
  }
  return receipts;
}

export function decodeServerAckWireBatch(batch: PackedWireBatch): ServerAckWireData[] {
  const count = serverAckReader.begin(batch);
  const acks: ServerAckWireData[] = new Array(count);
  for (let i = 0; i < count; i++) {
    const cls = serverAckReader.strOptional();
    const from = serverAckReader.jidOptional();
    const error = serverAckReader.strOptional();
    const timestamp = serverAckReader.f64();
    acks[i] = {
      id: serverAckReader.inline(),
      class: cls,
      from,
      timestamp: Number.isNaN(timestamp) ? undefined : timestamp,
      error,
    };
  }
  return acks;
}

// ---------------------------------------------------------------------------
// Batch builders (tests and replay tooling)
// ---------------------------------------------------------------------------

/**
 * Build a packed batch the way the Rust writer does. Every batch defines its
 * own caches and asks the reader to reset, so a fabricated batch decodes the
 * same regardless of what ran before it.
 */
class PackedBatchBuilder {
  private readonly stringSlots = new Map<string, number>();
  private readonly jidSlots = new Map<string, number>();
  private readonly newStrings: Array<[number, number]> = [];
  private readonly newJids: Array<[number, number, number, number, number, number]> = [];
  private readonly records: number[] = [];
  private readonly definitions: string[] = [];
  private readonly inline: string[] = [];
  private recordCount = 0;

  cacheStr(value: string): number {
    const existing = this.stringSlots.get(value);
    if (existing !== undefined) return existing;
    const slot = this.stringSlots.size;
    this.stringSlots.set(value, slot);
    this.newStrings.push([slot, utf8Encoder.encode(value).length]);
    this.definitions.push(value);
    return slot;
  }

  cacheStrOptional(value: string | undefined): number {
    return value === undefined ? 0 : this.cacheStr(value) + 1;
  }

  cacheJid(jid: WireJid): number {
    const key = [jid.user, jid.server, jid.agent, jid.device, jid.integrator].join(" ");
    const existing = this.jidSlots.get(key);
    if (existing !== undefined) return existing;
    const user = this.cacheStr(jid.user);
    const server = this.cacheStr(jid.server);
    const slot = this.jidSlots.size;
    this.jidSlots.set(key, slot);
    this.newJids.push([slot, user, server, jid.agent, jid.device, jid.integrator]);
    return slot;
  }

  cacheJidOptional(jid: WireJid | undefined): number {
    return jid === undefined ? 0 : this.cacheJid(jid) + 1;
  }

  u8(value: number): void {
    this.records.push(value & 255);
  }

  slot(value: number): void {
    this.records.push(value & 255, (value >> 8) & 255);
  }

  f64(value: number): void {
    const scratch = new DataView(new ArrayBuffer(8));
    scratch.setFloat64(0, value, true);
    for (let i = 0; i < 8; i++) this.records.push(scratch.getUint8(i));
  }

  writeInline(value: string): void {
    const length = utf8Encoder.encode(value).length;
    this.records.push(length & 255, (length >> 8) & 255);
    this.inline.push(value);
  }

  endRecord(): void {
    this.recordCount++;
  }

  finish(): PackedWireBatch {
    const stringBytes = utf8Encoder.encode(this.definitions.join("") + this.inline.join(""));
    const size =
      PACKED_HEADER_BYTES +
      this.newStrings.length * 4 +
      this.newJids.length * 11 +
      this.records.length +
      stringBytes.length;
    const buffer = new Uint8Array(size);
    const view = new DataView(buffer.buffer);
    view.setUint32(0, this.recordCount, true);
    view.setUint32(4, this.newStrings.length, true);
    view.setUint32(8, this.newJids.length, true);
    view.setUint32(12, stringBytes.length, true);
    view.setUint32(16, PACKED_FLAG_RESET_CACHES, true);

    let at = PACKED_HEADER_BYTES;
    for (const [slot, length] of this.newStrings) {
      view.setUint16(at, slot, true);
      view.setUint16(at + 2, length, true);
      at += 4;
    }
    for (const [slot, user, server, agent, device, integrator] of this.newJids) {
      view.setUint16(at, slot, true);
      view.setUint16(at + 2, user, true);
      view.setUint16(at + 4, server, true);
      view.setUint8(at + 6, agent);
      view.setUint16(at + 7, device, true);
      view.setUint16(at + 9, integrator, true);
      at += 11;
    }
    buffer.set(this.records, at);
    at += this.records.length;
    buffer.set(stringBytes, at);
    return buffer;
  }
}

/** Inverse of `decodeReceiptWireBatch`, for tests and replay tooling. */
export function encodeReceiptWireBatch(receipts: readonly ReceiptWireData[]): PackedWireBatch {
  const builder = new PackedBatchBuilder();
  for (const receipt of receipts) {
    const typeOther = typeof receipt.type !== "string";
    const chat = builder.cacheJid(receipt.source.chat);
    const sender = builder.cacheJid(receipt.source.sender);
    const senderAlt = builder.cacheJidOptional(receipt.source.sender_alt);
    const recipientAlt = builder.cacheJidOptional(receipt.source.recipient_alt);
    const broadcastListOwner = builder.cacheJidOptional(receipt.source.broadcast_list_owner);
    const recipient = builder.cacheJidOptional(receipt.source.recipient);
    const addressingMode = builder.cacheStrOptional(receipt.source.addressing_mode);
    const type = builder.cacheStr(
      typeof receipt.type === "string" ? receipt.type : receipt.type.Other,
    );
    builder.u8(
      (receipt.source.is_from_me ? RECEIPT_FLAG_FROM_ME : 0) |
        (receipt.source.is_group ? RECEIPT_FLAG_GROUP : 0) |
        (receipt.offline ? RECEIPT_FLAG_OFFLINE : 0) |
        (typeOther ? RECEIPT_FLAG_TYPE_OTHER : 0),
    );
    for (const slot of [
      chat,
      sender,
      senderAlt,
      recipientAlt,
      broadcastListOwner,
      recipient,
      addressingMode,
      type,
    ]) {
      builder.slot(slot);
    }
    builder.f64(receipt.timestamp);
    builder.u8(receipt.message_ids.length);
    for (const id of receipt.message_ids) builder.writeInline(id);
    builder.endRecord();
  }
  return builder.finish();
}

/** Inverse of `decodeServerAckWireBatch`, for tests and replay tooling. */
export function encodeServerAckWireBatch(acks: readonly ServerAckWireData[]): PackedWireBatch {
  const builder = new PackedBatchBuilder();
  for (const ack of acks) {
    builder.slot(builder.cacheStrOptional(ack.class));
    builder.slot(builder.cacheJidOptional(ack.from));
    builder.slot(builder.cacheStrOptional(ack.error));
    builder.f64(ack.timestamp ?? Number.NaN);
    builder.writeInline(ack.id);
    builder.endRecord();
  }
  return builder.finish();
}

// ---------------------------------------------------------------------------
// Event envelope
// ---------------------------------------------------------------------------

/** Segment tags, mirroring the Rust writer in `src/wire_batch.rs`. */
export const EVENT_SEGMENT_KIND_MESSAGE = 1;
export const EVENT_SEGMENT_KIND_RECEIPT = 2;
export const EVENT_SEGMENT_KIND_SERVER_ACK = 3;

/**
 * Byte layout of an event envelope, written by `EventWireEnvelope::finish`:
 *
 * ```text
 * header:  u32 segment_count | u32 reserved
 * segment: u32 kind | u32 byte_len | byte_len bytes | padding to 8
 * ```
 *
 * Payloads land 8-aligned so a message segment is still read as a view over the
 * envelope rather than copied out of it.
 */
const ENVELOPE_HEADER_BYTES = 8;
const ENVELOPE_SEGMENT_PREFIX_BYTES = 8;

/** One packed batch inside an envelope. */
export interface EventWireSegment {
  /** One of the `EVENT_SEGMENT_KIND_*` tags; names the codec for `batch`. */
  kind: number;
  /** Exactly the bytes the segment's own callback would have received. */
  batch: PackedWireBatch;
}

/**
 * Split an envelope into its segments, in the order the events arrived. Decode
 * each with the codec its kind names and deliver them in that order.
 */
export function decodeEventWireEnvelope(envelope: Uint8Array): EventWireSegment[] {
  if (envelope.byteLength < ENVELOPE_HEADER_BYTES) {
    throw new RangeError("event wire envelope is shorter than its header");
  }
  const view = new DataView(envelope.buffer, envelope.byteOffset, envelope.byteLength);
  const count = view.getUint32(0, true);
  const segments: EventWireSegment[] = new Array(count);
  let at = ENVELOPE_HEADER_BYTES;
  for (let i = 0; i < count; i++) {
    if (at + ENVELOPE_SEGMENT_PREFIX_BYTES > envelope.byteLength) {
      throw new RangeError("event wire envelope is truncated");
    }
    const kind = view.getUint32(at, true);
    const length = view.getUint32(at + 4, true);
    at += ENVELOPE_SEGMENT_PREFIX_BYTES;
    if (at + length > envelope.byteLength) {
      throw new RangeError("event wire envelope segment is truncated");
    }
    segments[i] = { kind, batch: envelope.subarray(at, at + length) };
    at = (at + length + 7) & ~7;
  }
  return segments;
}

/** Inverse of `decodeEventWireEnvelope`, for tests and replay tooling. */
export function encodeEventWireEnvelope(segments: readonly EventWireSegment[]): Uint8Array {
  let total = ENVELOPE_HEADER_BYTES;
  for (const { batch } of segments) {
    total = (total + ENVELOPE_SEGMENT_PREFIX_BYTES + batch.byteLength + 7) & ~7;
  }
  const envelope = new Uint8Array(total);
  const view = new DataView(envelope.buffer);
  view.setUint32(0, segments.length, true);
  let at = ENVELOPE_HEADER_BYTES;
  for (const { kind, batch } of segments) {
    view.setUint32(at, kind, true);
    view.setUint32(at + 4, batch.byteLength, true);
    at += ENVELOPE_SEGMENT_PREFIX_BYTES;
    envelope.set(batch, at);
    at = (at + batch.byteLength + 7) & ~7;
  }
  return envelope;
}
