//! Full WhatsApp client running in WASM.
//!
//! Wraps `whatsapp_rust::Client` with JS-provided adapters for
//! transport (WebSocket), storage (InMemory/JS), and HTTP (fetch).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use log::info;
use wasm_bindgen::prelude::*;
use whatsapp_rust::wacore::types::events::{Event, EventHandler, LazyHistorySync};
use whatsapp_rust::wacore_binary::jid::Jid;
use whatsapp_rust::{wacore, wacore_binary, waproto};

use crate::js_backend;
use crate::js_http::JsHttpClientAdapter;
use crate::js_keys;
use crate::js_time;
use crate::js_transport::JsTransportFactory;
use crate::runtime::WasmRuntime;
use crate::wire_batch::{
    BatchBuffer, CrossedBatch, EVENT_SEGMENT_KIND_MESSAGE, EVENT_SEGMENT_KIND_RECEIPT,
    EVENT_SEGMENT_KIND_SERVER_ACK, EventWireEnvelope, MessageWireBatch, PackedEventBatch,
    ReceiptWireBatch, ServerAckWireBatch,
};

thread_local! {
    /// Receivers signaled when a `Drop`-spawned cleanup task completes.
    /// `create_whatsapp_client` drains this before starting so a new client
    /// is never constructed while a previous client's async teardown still
    /// has tasks parked on JsFutures on the shared WASM heap. Event-driven —
    /// no timers.
    static PENDING_DROP_CLEANUPS: RefCell<Vec<oneshot::Receiver<()>>> =
        const { RefCell::new(Vec::new()) };
}

fn register_drop_cleanup() -> oneshot::Sender<()> {
    let (tx, rx) = oneshot::channel();
    PENDING_DROP_CLEANUPS.with(|p| p.borrow_mut().push(rx));
    tx
}

async fn drain_drop_cleanups() {
    loop {
        let drained: Vec<oneshot::Receiver<()>> =
            PENDING_DROP_CLEANUPS.with(|p| std::mem::take(&mut *p.borrow_mut()));
        if drained.is_empty() {
            break;
        }
        let _ = futures::future::join_all(drained).await;
    }
}

// ---------------------------------------------------------------------------
// TypeScript type declarations
// ---------------------------------------------------------------------------

// These opaque host types reference declarations generated directly from the
// core Serde schema. They add no runtime DTO or conversion implementation; the
// method boundary below deserializes/serializes the core types themselves.
#[wasm_bindgen(typescript_custom_section)]
const _TS_BINARY_NODE: &str = r#"
/** Neutral binary stanza representation accepted by the client boundary. */
export interface BinaryNode {
  tag: string;
  attrs: Record<string, string>;
  content?: BinaryNode[] | string | Uint8Array;
}
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "BinaryNode")]
    pub type JsBinaryNode;

    #[wasm_bindgen(typescript_type = "BinaryNode[]")]
    pub type JsBinaryNodeArray;

    #[wasm_bindgen(typescript_type = "UsyncQuery")]
    pub type JsUsyncQuery;

    #[wasm_bindgen(typescript_type = "UsyncResponse")]
    pub type JsUsyncResponse;
}

// ---------------------------------------------------------------------------
// Event definition — SINGLE SOURCE OF TRUTH
// ---------------------------------------------------------------------------
//
// `bridge_events!` generates BOTH from one definition:
//   1. The TypeScript `WhatsAppEvent` union type (typescript_custom_section)
//   2. The `event_to_js` Rust dispatch function
//
// To add a new serializable event: add ONE line in `serialize { }`.
// To add a new special event: add in `special { }` AND handle in `event_to_js_special`.

/// Helper: generates one WhatsAppEvent TS union variant line.
macro_rules! ev {
    ($name:literal, $ts_type:literal) => {
        concat!("  | { type: '", $name, "'; data: ", $ts_type, " }\n")
    };
}

thread_local! {
    /// Event names, interned once per thread alongside the envelope keys in
    /// [`crate::js_keys`]. Bounded by the event enum.
    static EVENT_NAME_CACHE: RefCell<HashMap<&'static str, JsValue>> =
        RefCell::new(HashMap::new());
}

fn make_js_event(event_type: &'static str, data: &JsValue) -> Result<JsValue, JsValue> {
    let event = js_sys::Object::new();
    let name = EVENT_NAME_CACHE.with(|c| {
        c.borrow_mut()
            .entry(event_type)
            .or_insert_with(|| JsValue::from_str(event_type))
            .clone()
    });
    js_keys::set(&event, &js_keys::EVENT_TYPE_KEY, &name)?;
    js_keys::set(&event, &js_keys::EVENT_DATA_KEY, data)?;
    Ok(event.into())
}

macro_rules! bridge_events {
    (
        serialize {
            $( $variant:ident => $name:literal => $ts_type:literal ),* $(,)?
        }
        special {
            $( $xname:literal => $xts:literal ),* $(,)?
        }
    ) => {
        // Generate WhatsAppEvent TypeScript type
        #[wasm_bindgen(typescript_custom_section)]
        const _TS_WHATSAPP_EVENT: &str = concat!(
            "export type WhatsAppEvent =\n",
            $( ev!($name, $ts_type), )*
            $( ev!($xname, $xts), )*
            ";\n",
        );

        // Generate event_to_js dispatch (JS-specific, existing path)
        fn event_to_js(event: &Event) -> Result<JsValue, JsValue> {
            let (event_type, data) = match event {
                $( Event::$variant(data) => ($name, crate::proto::to_js_value(data)?), )*
                other => return event_to_js_special(other),
            };
            make_js_event(event_type, &data)
        }
    };
}

bridge_events! {
    serialize {
        // Variant              => "js_name"                       => "TsDataType"
        Receipt                  => "receipt"                       => "Receipt",
        ServerAck                => "server_ack"                    => "ServerAck",
        UndecryptableMessage     => "undecryptable_message"         => "UndecryptableMessage",
        ChatPresence             => "chat_presence"                 => "ChatPresenceUpdate",
        Presence                 => "presence"                      => "PresenceUpdate",
        PictureUpdate            => "picture_update"                => "PictureUpdate",
        UserAboutUpdate          => "user_about_update"             => "UserAboutUpdate",
        ContactUpdated           => "contact_updated"               => "ContactUpdated",
        ContactNumberChanged     => "contact_number_changed"        => "ContactNumberChanged",
        ContactSyncRequested     => "contact_sync_requested"        => "ContactSyncRequested",
        GroupUpdate              => "group_update"                  => "GroupUpdate",
        ContactUpdate            => "contact_update"                => "ContactUpdate",
        PushNameUpdate           => "push_name_update"              => "PushNameUpdate",
        SelfPushNameUpdated      => "self_push_name_updated"        => "SelfPushNameUpdated",
        PinUpdate                => "pin_update"                    => "PinUpdate",
        MuteUpdate               => "mute_update"                  => "MuteUpdate",
        ArchiveUpdate            => "archive_update"                => "ArchiveUpdate",
        StarUpdate               => "star_update"                   => "StarUpdate",
        MarkChatAsReadUpdate     => "mark_chat_as_read_update"      => "MarkChatAsReadUpdate",
        DeleteChatUpdate         => "delete_chat_update"            => "DeleteChatUpdate",
        ClearChatUpdate          => "clear_chat_update"             => "ClearChatUpdate",
        UserStatusMuteUpdate     => "user_status_mute_update"       => "UserStatusMuteUpdate",
        DeleteMessageForMeUpdate => "delete_message_for_me_update"  => "DeleteMessageForMeUpdate",
        LabelEditUpdate          => "label_edit_update"             => "LabelEditUpdate",
        LabelAssociationUpdate   => "label_association_update"      => "LabelAssociationUpdate",
        OfflineSyncPreview       => "offline_sync_preview"          => "OfflineSyncPreview",
        OfflineSyncCompleted     => "offline_sync_completed"        => "OfflineSyncCompleted",
        DirtyState               => "dirty_state"                    => "{ dirty_type: DirtyType; timestamp?: number | null }",
        DeviceListUpdate         => "device_list_update"            => "DeviceListUpdate",
        IdentityChange           => "identity_change"               => "IdentityChange",
        BusinessStatusUpdate     => "business_status_update"        => "BusinessStatusUpdate",
        TemporaryBan             => "temporary_ban"                 => "TemporaryBan",
        ConnectFailure           => "connect_failure"               => "ConnectFailure",
        StreamError              => "stream_error"                  => "StreamError",
        DisappearingModeChanged  => "disappearing_mode_changed"     => "DisappearingModeChanged",
        NewsletterLiveUpdate     => "newsletter_live_update"        => "NewsletterLiveUpdate",
        IncomingCall             => "incoming_call"                 => "IncomingCall",
        MissedCall               => "missed_call"                   => "MissedCall",
        CallEndedElsewhere       => "call_ended_elsewhere"          => "CallEndedElsewhere",
        MexNotification          => "mex_notification"               => "MexNotification",
        PairingCodeRefresh       => "pairing_code_refresh"          => "PairingCodeRefresh",
        PairPasskeyRequest       => "pair_passkey_request"          => "PairPasskeyRequest",
        PairPasskeyConfirmation  => "pair_passkey_confirmation"     => "PairPasskeyConfirmation",
        PairPasskeyError         => "pair_passkey_error"            => "PairPasskeyError",
    }
    special {
        // "js_name"                         => "TsDataType"
        "connected"                       => "Record<string, never>",
        "disconnected"                    => "Record<string, never>",
        "qr"                              => "{ code: string; timeout: number }",
        "pairing_code"                    => "{ code: string; timeout: number }",
        "pair_success"                    => "{ id: string; lid: string; business_name: string; platform: string }",
        "pair_error"                      => "{ id: string; lid: string; business_name: string; platform: string; error: string }",
        "logged_out"                      => "{ on_connect: boolean; reason: string }",
        "message"                         => "{ message: Record<string, unknown>; info: MessageInfo & { is_view_once: boolean } }",
        "notification"                    => "{ tag: string; attrs: Record<string, string>; content?: unknown }",
        "stream_replaced"                 => "Record<string, never>",
        "qr_scanned_without_multidevice"  => "Record<string, never>",
        "client_outdated"                 => "Record<string, never>",
        "raw_node"                        => "{ tag: string; attrs: Record<string, string>; content?: unknown }",
        // The bridge itself never emits `message`/`history_sync` events — both
        // cross the boundary as wire batches (`onMessageBatch` /
        // `onHistorySyncBatch`). The union entries describe the host-side
        // reconstruction: hosts decode the wire payloads with their own codec
        // and rebuild these shapes for their downstream consumers.
        "history_sync"                    => "import('./proto-types').proto.IHistorySync & { syncType: number; chunkOrder?: number; progress?: number; peerDataRequestSessionId?: string }",
    }
}

#[wasm_bindgen(typescript_custom_section)]
const _TS_CLIENT_CONFIG: &str = r#"
export interface WhatsAppClientConfig {
  transport: JsTransportCallbacks;
  httpClient: JsHttpClientConfig;
  onEvent?: WhatsAppEventHandler;
}

/**
 * Typed event sink. Message and history-sync events cross the boundary as
 * protobuf wire bytes only: the host decodes them with its own codec, so the
 * bridge never materializes an intermediate reflected JS tree (and never
 * compiles the Rust->JS serializers for those proto graphs). A handler that
 * omits `onMessageBatch`/`onHistorySyncBatch` has those events dropped with an
 * error log; every other event kind continues through `onEvent`.
 */
export interface WhatsAppEventCallbacks {
  onEvent(event: WhatsAppEvent): void;
  /**
   * Protobuf-wire message path. The bridge packs a bounded ordered group of
   * messages — payloads and metadata alike — into one flat buffer. Decode it
   * with `decodeMessageWireBatch`.
   */
  onMessageBatch(batch: MessageWireBatch): void;
  /**
   * Optional host-interest filter for conversation records. When present, the
   * bridge still walks every history payload and emits its final metadata, but
   * materializes conversation wire bytes only for the listed numeric sync
   * types. Omit it for the backward-compatible "all types" behavior.
   */
  historySyncConversationTypes?: readonly number[];
  /**
   * Protobuf-wire history-sync path. Conversation entries cross as wire bytes
   * and the non-conversation remainder (pushnames, mappings, settings, ...)
   * crosses as one encoded `proto.HistorySync` payload in `remainderData`.
   * Return the number of malformed entries skipped by the host, if any.
   */
  onHistorySyncBatch(batch: HistorySyncWireBatch): number | void;
  /**
   * Optional packed receipt path: adjacent `receipt` events coalesce into one
   * flat buffer (decode with `decodeReceiptWireBatch`) instead of one
   * reflected object per event. Without it, receipts use `onEvent`.
   * Decode the batch fully before calling back into the client: the reader
   * walks the buffer in order and shares cached JID objects across events.
   */
  onReceiptBatch?(batch: ReceiptWireBatch): void;
  /**
   * Optional packed server-ack path, analogous to `onReceiptBatch`; decode
   * with `decodeServerAckWireBatch`. Without it, acks use `onEvent`.
   */
  onServerAckBatch?(batch: ServerAckWireBatch): void;
  /**
   * Borrowing form of `onReceiptBatch`, and the whole of the opt-in: declaring
   * it replaces `onReceiptBatch` as the receipt sink and lets the bridge hand
   * every batch out of one buffer it reuses.
   *
   * The batch is therefore a window on shared memory, valid ONLY for the
   * synchronous duration of the call: implementations MUST decode or copy it
   * before returning, and MUST NOT retain it, pass it to an async consumer, or
   * return a Promise. `decodeReceiptWireBatch` satisfies that on its own, since
   * it materializes every field and keeps no view over the buffer, so the usual
   * body is a decode plus whatever the host does with the result. A callback
   * caught handing back something promise-like, or letting an exception escape,
   * drops every borrowing callback of every packed kind back to a buffer per
   * batch for the rest of the session, before the buffer is reused, so the
   * window it kept stays intact.
   *
   * `onReceiptBatch` remains the copying path for every host that does not opt
   * in, and a batch too large for the shared buffer gets its own regardless.
   * There is no borrowing form of `onMessageBatch`: `decodeMessageWireBatch`
   * returns views over the batch, so reuse there would alias the decoded result
   * and not just the buffer.
   */
  onReceiptBatchBorrowed?(batch: ReceiptWireBatch): void;
  /** Borrowing form of `onServerAckBatch`, under the contract above. */
  onServerAckBatchBorrowed?(batch: ServerAckWireBatch): void;
  /**
   * Optional coalescing path. A live message produces up to three batches (its
   * own, its receipt, its ack); with this method the ones the dispatch loop
   * already holds cross together as one envelope of tagged segments instead of
   * one crossing each. Split it with `decodeEventWireEnvelope` and hand each
   * segment to the codec its kind names, in order. Nothing is ever held back
   * waiting for a companion event, so a batch with no company still arrives
   * through its own callback above, in the buffer that callback negotiated.
   *
   * There is no borrowing form, for the reason `onMessageBatch` has none: an
   * envelope may carry a message segment, whose decode returns views over the
   * buffer.
   */
  onEventBatch?(batch: EventWireEnvelope): void;
}

/**
 * A packed batch is one flat buffer: header, records and string bytes. Decode it
 * with the matching codec (`decodeMessageWireBatch`, `decodeReceiptWireBatch`,
 * `decodeServerAckWireBatch`), which reads views over the buffer instead of
 * copying. The bare typed array crosses rather than a wrapper object: a message
 * produces up to three batches, so an object per batch is three constructions
 * and three property writes of pure overhead.
 */
export type MessageWireBatch = Uint8Array;
export type ReceiptWireBatch = Uint8Array;
export type ServerAckWireBatch = Uint8Array;

/**
 * Several packed batches in one buffer, each tagged with the kind that names
 * its codec. Split it with `decodeEventWireEnvelope`; every segment is byte for
 * byte the batch its own callback would have received.
 */
export type EventWireEnvelope = Uint8Array;

export type HistorySyncWireBatch = {
  /** Concatenated Conversation protobuf payloads for this bounded batch. */
  conversationData: Uint8Array;
  /** Start offsets into conversationData, followed by its final byte length. */
  conversationOffsets: Uint32Array;
  /**
   * Encoded `proto.HistorySync` carrying every non-conversation field. Present
   * only on the final batch of a chunk.
   */
  remainderData?: Uint8Array;
  syncType: number;
  chunkOrder?: number;
  progress?: number;
  peerDataRequestSessionId?: string;
  batchIndex: number;
  isFinalBatch: boolean;
};

/**
 * Plain functions remain supported for control-plane events (pairing, QR,
 * connection lifecycle); message and history-sync delivery requires the
 * callback-object form above.
 */
export type WhatsAppEventHandler =
  | ((event: WhatsAppEvent) => void)
  | WhatsAppEventCallbacks;

/**
 * JS storage callbacks for the persistent backend.
 *
 * The boundary is a two-level namespaced key/value store: `store` is one of the
 * fixed STORE_* namespaces (e.g. "session", "msg_secret", "lid_mapping") and
 * `key` is an opaque, namespace-scoped id; values are raw bytes.
 *
 * Only `get`/`set`/`delete` are MANDATORY — a 3-method store keeps working
 * exactly as before. The remaining methods are OPTIONAL performance/structural
 * primitives the core feature-detects (by handle presence) and uses when the
 * host provides them:
 *   - `setMany`/`deleteMany` collapse N per-key FFI crossings into one (this is
 *     what turns a ~20k-secret history-sync write from 20k awaits into a single
 *     batched call).
 *   - `listKeys`/`listEntries` let the core enumerate a namespace directly,
 *     which lets it DROP its hand-maintained meta-index lists (msg_secret_keys,
 *     tc_token_jids, …). A host that cannot enumerate a category (e.g. an
 *     id-addressed external key store) simply omits them; the core then keeps its
 *     self-maintained index for that backend.
 *   - `deletePrefix` accelerates unconditional bulk clears.
 *
 * `capabilities` is read ONCE at init. Omit it (or a field) and the core treats
 * the corresponding primitive as absent. A capability declared `true` MUST have
 * its method(s) present and working.
 */
export interface JsStoreCallbacks {
  /** Read one value by (store, key). Null/undefined if absent. MANDATORY. */
  get(store: string, key: string): Promise<Uint8Array | null>;
  /** Write one value by (store, key). MANDATORY. */
  set(store: string, key: string, value: Uint8Array): Promise<void>;
  /** Delete one key. No-op if absent. MANDATORY. */
  delete(store: string, key: string): Promise<void>;

  /**
   * Write many [key, value] pairs into ONE store in a single call. Entries are
   * tuples (so keys may contain any character). Best-effort: if the medium has
   * no cross-key atomicity (file-per-key) it MUST still apply every entry and
   * fail-fast on error so the core can retry (writes are idempotent by key).
   * Empty array is a valid no-op.
   */
  setMany?(store: string, entries: [key: string, value: Uint8Array][]): Promise<void>;

  /** Read many keys from ONE store; one entry per FOUND key, any order. */
  getMany?(store: string, keys: string[]): Promise<[key: string, value: Uint8Array][]>;

  /** Delete many keys from ONE store in a single call. Missing keys ignored. */
  deleteMany?(store: string, keys: string[]): Promise<void>;

  /** Enumerate live keys in `store` (optionally prefix-filtered). Unordered. */
  listKeys?(store: string, prefix?: string): Promise<string[]>;

  /**
   * Like listKeys but returns [key, value] pairs, so the core can inspect the
   * embedded timestamp prefix for delete-expired sweeps without N follow-up
   * gets. If absent but listKeys exists, the core falls back to listKeys+getMany.
   */
  listEntries?(store: string, prefix?: string): Promise<[key: string, value: Uint8Array][]>;

  /** Delete every key in `store` starting with `prefix`. Returns count removed. */
  deletePrefix?(store: string, prefix: string): Promise<number>;

  /** Static capability declaration, read once at init. Omitted => all false. */
  capabilities?: {
    /** setMany/getMany/deleteMany are implemented. */
    batch?: boolean;
    /** listKeys/listEntries reliably enumerate a namespace. */
    enumerate?: boolean;
    /** deletePrefix is implemented. */
    prefixDelete?: boolean;
  };

  /** Optional durability barrier (flush pending writes). */
  flush?(): Promise<void>;
}

/**
 * Initialize the WASM engine. Call once before creating clients.
 * @param logger Optional pino-compatible logger.
 * @param crypto Optional native crypto callbacks — when provided, AES/HMAC
 *               primitives delegate to the host (e.g. `node:crypto`). Falls
 *               back to the Rust-soft implementation if omitted.
 */
export function initWasmEngine(logger?: any, crypto?: JsCryptoCallbacks): void;

/**
 * Create a full WhatsApp client running in WASM.
 *
 * @param transport_config WebSocket transport callbacks (connect/send/disconnect)
 * @param http_config HTTP client callbacks (execute via fetch)
 * @param on_event Optional typed event sink — receives WhatsApp events in order
 * @param store Optional JS storage callbacks — if provided, enables persistent storage
 * @param cache_config Optional cache TTL/capacity and custom store overrides
 * @param version Optional [major, minor, patch] WhatsApp Web version override
 * @param wanted_pre_key_count Optional pre-key upload batch size (default 812);
 *   clamped to the protocol-safe range at upload time. Smaller batches reduce
 *   memory pressure on embedded/WASM hosts.
 */
export function createWhatsAppClient(
  transport_config: JsTransportCallbacks,
  http_config: JsHttpClientConfig,
  on_event?: WhatsAppEventHandler | null,
  store?: JsStoreCallbacks | null,
  cache_config?: CacheConfig | null,
  version?: readonly [number, number, number] | null,
  wanted_pre_key_count?: number | null,
): Promise<WasmWhatsAppClient>;

/** Cache entry configuration. */
export interface CacheEntryConfig {
  ttlSecs?: number;
  capacity?: number;
  store?: JsCacheStore;
}

/** Custom cache backend. */
export interface JsCacheStore {
  get(namespace: string, key: string): Promise<Uint8Array | null>;
  set(namespace: string, key: string, value: Uint8Array, ttlSecs?: number): Promise<void>;
  delete(namespace: string, key: string): Promise<void>;
  clear(namespace: string): Promise<void>;
}

/** Cache configuration — all fields optional. */
export interface CacheConfig {
  store?: JsCacheStore;
  group?: CacheEntryConfig;
  device?: CacheEntryConfig;
  deviceRegistry?: CacheEntryConfig;
  lidPn?: CacheEntryConfig;
  retriedGroupMessages?: CacheEntryConfig;
  recentMessages?: CacheEntryConfig;
  messageRetry?: CacheEntryConfig;
}

// Augment WasmWhatsAppClient with methods that need skip_typescript
// (Record returns can't be expressed by wasm-bindgen)
interface WasmWhatsAppClient {
  /** Fetch all groups the user is participating in. */
  groupFetchAllParticipating(): Promise<Record<string, GroupMetadataResult>>;
  /** Fetch all parent groups the user is participating in. */
  communityFetchAllParticipating(): Promise<Record<string, GroupMetadataResult>>;
  /** Fetch user info for one or more JIDs. */
  fetchUserInfo(jids: string[]): Promise<Record<string, UserInfoResult>>;
}
"#;

// ---------------------------------------------------------------------------
// JS event handler bridge
// ---------------------------------------------------------------------------

/// Bridges Rust events to a JS callback function via an ordered channel.
///
/// Raw `Arc<Event>`s are sent through an async channel and SERIALIZED in the
/// single consumer loop (not in `handle_event`), which guarantees delivery
/// order (unlike per-event `spawn_local` which does not) AND keeps each
/// serialized JS object tree short-lived: a JsValue is built right before its
/// callback and is collectable right after it returns. Pre-serializing in
/// `handle_event` made every queued event's full JS tree coexist in the
/// channel during bursts (history sync, offline replay) — the surviving
/// objects got tenured into V8's old space, permanently growing heapTotal
/// (committed pages V8 never returns to the OS).
struct JsEventHandler {
    event_tx: async_channel::Sender<Arc<Event>>,
}

crate::wasm_send_sync!(JsEventHandler);

/// Maximum number of consecutive JS callbacks while more bridge work is
/// immediately available. Once the queue drains, `Receiver::recv().await`
/// already yields naturally, so scheduling another macrotask would only add
/// latency and allocations to live traffic.
const EVENT_CALLBACK_BUDGET: u32 = 50;
const EVENT_CHANNEL_CAPACITY: usize = 16_384;
/// Upper bound for object trees handed across the JS/WASM boundary at once.
/// This is a host-boundary resource limit shared by every batched event path,
/// not a WhatsApp protocol rule.
const EVENT_BATCH_CAPACITY: usize = 32;
/// Whether an isolated live message spends one cooperative I/O turn trying to
/// collect an adjacent frame before dispatching. Measured: disabling it drops
/// coalescing to 1.00 messages per batch and the extra batches cost more than
/// the saved macrotask.
const MESSAGE_SINGLETON_COLLECT_TURN: bool = true;
/// History conversations expand into dozens of JS objects each. A smaller
/// boundary limits each synchronous callback's object tree. The whole chunk is
/// drained without admitting more I/O; one macrotask yield happens only after
/// its compressed event has been released.
const HISTORY_SYNC_BATCH_MAX_CONVERSATIONS: usize = 16;
/// Keep the packed wire buffer within one linear-memory page when entries are
/// small enough. A single larger conversation remains indivisible and is sent
/// alone; the count ceiling still bounds tiny-entry callback latency.
const HISTORY_SYNC_BATCH_MAX_BYTES: usize = crate::WASM_PAGE_BYTES;
const EVENT_CALLBACK_METHOD: &str = "onEvent";
const MESSAGE_BATCH_CALLBACK_METHOD: &str = "onMessageBatch";
const HISTORY_SYNC_BATCH_CALLBACK_METHOD: &str = "onHistorySyncBatch";
const RECEIPT_BATCH_CALLBACK_METHOD: &str = "onReceiptBatch";
const SERVER_ACK_BATCH_CALLBACK_METHOD: &str = "onServerAckBatch";
const RECEIPT_BATCH_BORROWED_CALLBACK_METHOD: &str = "onReceiptBatchBorrowed";
const SERVER_ACK_BATCH_BORROWED_CALLBACK_METHOD: &str = "onServerAckBatchBorrowed";
const EVENT_BATCH_CALLBACK_METHOD: &str = "onEventBatch";
const HISTORY_SYNC_CONVERSATION_TYPES_FIELD: &str = "historySyncConversationTypes";

#[inline]
fn history_sync_wire_batch_should_flush(
    conversation_count: usize,
    buffered_bytes: usize,
    next_conversation_bytes: usize,
) -> bool {
    conversation_count >= HISTORY_SYNC_BATCH_MAX_CONVERSATIONS
        || (buffered_bytes != 0
            && buffered_bytes.saturating_add(next_conversation_bytes)
                > HISTORY_SYNC_BATCH_MAX_BYTES)
}

#[inline]
fn history_sync_wire_batch_next_capacity(current: usize, required: usize) -> usize {
    if current >= required {
        return current;
    }

    if current == 0 && required != 0 {
        // Estimate a complete batch from the first observed entry. Stay
        // strictly below the byte ceiling so allocator metadata/rounding does
        // not turn a page-sized request into a two-page `memory.grow`.
        let sub_page_budget = HISTORY_SYNC_BATCH_MAX_BYTES.saturating_sub(1);
        let estimated_count =
            (sub_page_budget / required).clamp(1, HISTORY_SYNC_BATCH_MAX_CONVERSATIONS);
        return required.saturating_mul(estimated_count);
    }

    // Preserve amortized geometric growth while a doubled buffer stays inside
    // the batch budget. Near the limit, reserve only the observed payload: a
    // speculative full-page allocation can itself force `memory.grow`.
    current
        .checked_mul(2)
        .filter(|&doubled| doubled >= required && doubled <= HISTORY_SYNC_BATCH_MAX_BYTES)
        .unwrap_or(required)
}

/// Whether `value` defers work the way the borrow contract forbids.
///
/// Thenable rather than `instanceof Promise`: the latter is realm-local, so it
/// would clear a promise built in another realm, and it would clear a bare
/// thenable. Both defer the host's decode past the callback, which is the thing
/// being checked for. Functions count, being objects that can carry a `then`.
/// Reached only when the callback returned one of those, which a conforming
/// `void` callback never does.
fn is_thenable(value: &JsValue) -> bool {
    (value.is_object() || value.is_function())
        && js_sys::Reflect::get(value, &"then".into()).is_ok_and(|then| then.is_function())
}

/// Registered delivery for one packed batch kind: the copying callback and,
/// when the host opted in, the borrowing one.
#[derive(Default)]
struct PackedBatchChannelState {
    copying: Option<js_sys::Function>,
    borrowing: Option<js_sys::Function>,
}

impl PackedBatchChannelState {
    /// The registered sink. Opting in replaces the copying callback rather than
    /// supplementing it, so a host never has to keep both in step.
    fn target(&self) -> Option<&js_sys::Function> {
        self.borrowing.as_ref().or(self.copying.as_ref())
    }

    fn is_registered(&self) -> bool {
        self.target().is_some()
    }

    /// Whether the next batch may cross in the shared buffer. Nothing between
    /// this and [`Self::call`] can change the answer: the dispatch loop does
    /// not await in between.
    fn borrows(&self) -> bool {
        self.borrowing.is_some() && !crate::wire_batch::borrowed_batches_revoked()
    }

    fn buffer(&self) -> BatchBuffer {
        if self.borrows() {
            BatchBuffer::Borrowed
        } else {
            BatchBuffer::Owned
        }
    }

    /// Deliver a batch crossed in the buffer [`Self::buffer`] picked.
    ///
    /// A borrowing callback that hands back something promise-like, or throws,
    /// has not finished with the window it was given, so the borrow is revoked.
    /// Revoking here, after that call and before the shared buffer is written
    /// again, is what keeps the window that callback kept from ever being
    /// rewritten. Every borrowed delivery is checked, not just the first: a
    /// callback that goes async down only one of its branches is precisely the
    /// one a single check would clear and then miss.
    fn call(&self, receiver: &JsValue, kind: &str, batch: &JsValue) -> Result<JsValue, JsValue> {
        let borrowed = self.borrows();
        let result = self
            .target()
            .expect("packed callback checked before dispatch")
            .call1(receiver, batch);
        // A callback that threw did not finish with the window either.
        let deferred = result.as_ref().map_or(true, is_thenable);
        if borrowed && deferred {
            crate::wire_batch::revoke_borrowed_batches();
            log::error!(
                "{kind} borrowing batch callback did not consume the batch synchronously; falling back to a buffer per batch"
            );
        }
        result
    }
}

/// Parsed once at client creation so the hot dispatch loop never performs
/// reflective method lookup. A plain function is the legacy control-plane
/// shape; message and history-sync delivery requires the object form with the
/// wire-batch methods (their events are dropped with an error log otherwise).
struct JsEventCallbacks {
    receiver: JsValue,
    on_event: js_sys::Function,
    on_message_batch: Option<js_sys::Function>,
    on_history_sync_batch: Option<js_sys::Function>,
    receipt_channel: PackedBatchChannelState,
    server_ack_channel: PackedBatchChannelState,
    on_event_batch: Option<js_sys::Function>,
    /// Bitset of host-requested numeric sync types; `None` preserves the legacy
    /// behavior of materializing conversations for every type.
    history_sync_conversation_types: Option<u128>,
}

impl JsEventCallbacks {
    fn from_js(value: JsValue) -> Result<Self, crate::errors::BridgeError> {
        if value.is_function() {
            return Ok(Self {
                receiver: JsValue::NULL,
                on_event: value.unchecked_into(),
                on_message_batch: None,
                on_history_sync_batch: None,
                receipt_channel: PackedBatchChannelState::default(),
                server_ack_channel: PackedBatchChannelState::default(),
                on_event_batch: None,
                history_sync_conversation_types: None,
            });
        }

        if !value.is_object() || value.is_null() {
            return Err(crate::errors::invalid_arg(
                "on_event",
                "must be a function or an object with an onEvent method",
            ));
        }

        let on_event = js_sys::Reflect::get(&value, &EVENT_CALLBACK_METHOD.into())
            .map_err(|_| {
                crate::errors::invalid_arg("on_event", "could not read the onEvent method")
            })?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| crate::errors::invalid_arg("on_event.onEvent", "must be a function"))?;

        let on_message_batch = Self::optional_method(&value, MESSAGE_BATCH_CALLBACK_METHOD)?;
        let on_history_sync_batch =
            Self::optional_method(&value, HISTORY_SYNC_BATCH_CALLBACK_METHOD)?;
        let receipt_channel = PackedBatchChannelState {
            copying: Self::optional_method(&value, RECEIPT_BATCH_CALLBACK_METHOD)?,
            borrowing: Self::optional_method(&value, RECEIPT_BATCH_BORROWED_CALLBACK_METHOD)?,
        };
        let server_ack_channel = PackedBatchChannelState {
            copying: Self::optional_method(&value, SERVER_ACK_BATCH_CALLBACK_METHOD)?,
            borrowing: Self::optional_method(&value, SERVER_ACK_BATCH_BORROWED_CALLBACK_METHOD)?,
        };
        let on_event_batch = Self::optional_method(&value, EVENT_BATCH_CALLBACK_METHOD)?;
        let history_sync_conversation_types =
            Self::optional_history_sync_conversation_types(&value)?;
        // Surface the contract gap at registration time: dispatch drops these
        // events later, and a host that only learns from per-event error logs
        // under load has a much worse debugging experience.
        if on_message_batch.is_none() {
            log::warn!("event callbacks lack onMessageBatch; message events will be dropped");
        }
        if on_history_sync_batch.is_none() {
            log::warn!(
                "event callbacks lack onHistorySyncBatch; history-sync events will be dropped"
            );
        }

        Ok(Self {
            receiver: value,
            on_event,
            on_message_batch,
            on_history_sync_batch,
            receipt_channel,
            server_ack_channel,
            on_event_batch,
            history_sync_conversation_types,
        })
    }

    fn optional_history_sync_conversation_types(
        receiver: &JsValue,
    ) -> Result<Option<u128>, crate::errors::BridgeError> {
        let value = js_sys::Reflect::get(receiver, &HISTORY_SYNC_CONVERSATION_TYPES_FIELD.into())
            .map_err(|_| {
            crate::errors::invalid_arg("on_event", "could not read historySyncConversationTypes")
        })?;
        if value.is_null() || value.is_undefined() {
            return Ok(None);
        }
        if !js_sys::Array::is_array(&value) {
            return Err(crate::errors::invalid_arg(
                "on_event.historySyncConversationTypes",
                "must be an array of non-negative integer sync types",
            ));
        }

        let mut types = 0u128;
        for value in js_sys::Array::from(&value).iter() {
            let Some(value) = value.as_f64() else {
                return Err(crate::errors::invalid_arg(
                    "on_event.historySyncConversationTypes",
                    "entries must be non-negative integer sync types",
                ));
            };
            if !value.is_finite()
                || value.fract() != 0.0
                || value < 0.0
                || value >= f64::from(u128::BITS)
            {
                return Err(crate::errors::invalid_arg(
                    "on_event.historySyncConversationTypes",
                    format!("entries must be integers in 0..{}", u128::BITS),
                ));
            }
            types |= 1u128 << value as u32;
        }
        Ok(Some(types))
    }

    fn optional_method(
        receiver: &JsValue,
        method: &'static str,
    ) -> Result<Option<js_sys::Function>, crate::errors::BridgeError> {
        let value = js_sys::Reflect::get(receiver, &method.into()).map_err(|_| {
            crate::errors::invalid_arg(
                "on_event",
                format!("could not read the optional {method} method"),
            )
        })?;
        if value.is_null() || value.is_undefined() {
            return Ok(None);
        }
        value.dyn_into::<js_sys::Function>().map(Some).map_err(|_| {
            crate::errors::invalid_arg(format!("on_event.{method}"), "must be a function")
        })
    }

    fn call_event(&self, event: &JsValue) -> Result<JsValue, JsValue> {
        self.on_event.call1(&self.receiver, event)
    }

    fn call_history_sync_batch(&self, batch: &JsValue) -> Result<JsValue, JsValue> {
        debug_assert!(self.on_history_sync_batch.is_some());
        self.on_history_sync_batch
            .as_ref()
            .expect("history-sync callback checked before dispatch")
            .call1(&self.receiver, batch)
    }

    fn call_message_batch(&self, batch: &JsValue) -> Result<JsValue, JsValue> {
        debug_assert!(self.on_message_batch.is_some());
        self.on_message_batch
            .as_ref()
            .expect("message callback checked before dispatch")
            .call1(&self.receiver, batch)
    }

    fn supports_history_sync_batching(&self) -> bool {
        self.on_history_sync_batch.is_some()
    }

    fn supports_message_wire_batching(&self) -> bool {
        self.on_message_batch.is_some()
    }

    fn call_event_batch(&self, batch: &JsValue) -> Result<JsValue, JsValue> {
        debug_assert!(self.on_event_batch.is_some());
        self.on_event_batch
            .as_ref()
            .expect("event-envelope callback checked before dispatch")
            .call1(&self.receiver, batch)
    }

    /// Whether this event's batch may be buffered into an envelope: only the
    /// kinds the host declared a packed callback for, since an envelope segment
    /// is the same buffer that callback would have received.
    fn buffers_into_envelope(&self, event: &Event) -> bool {
        match event {
            Event::Messages(_) => self.supports_message_wire_batching(),
            Event::Receipt(_) => self.receipt_channel.is_registered(),
            Event::ServerAck(_) => self.server_ack_channel.is_registered(),
            _ => false,
        }
    }

    /// The delivery a lone segment falls back to, which is the one it would
    /// have used had the run never been buffered.
    fn channel_of_kind(&self, kind: u32) -> Option<(&'static str, &PackedBatchChannelState)> {
        match kind {
            EVENT_SEGMENT_KIND_RECEIPT => Some((ReceiptWireBatch::KIND, &self.receipt_channel)),
            EVENT_SEGMENT_KIND_SERVER_ACK => {
                Some((ServerAckWireBatch::KIND, &self.server_ack_channel))
            }
            _ => None,
        }
    }

    fn wants_history_sync_conversations(&self, sync_type: i32) -> bool {
        let Some(types) = self.history_sync_conversation_types else {
            return true;
        };
        let Ok(sync_type) = u32::try_from(sync_type) else {
            return false;
        };
        sync_type < u128::BITS && types & (1u128 << sync_type) != 0
    }
}

#[derive(Default)]
struct EventDispatchBudget {
    consecutive_callbacks: u32,
}

impl EventDispatchBudget {
    /// Record one callback and report whether the consumer should yield.
    ///
    /// `work_remains` covers both another message in the current core batch
    /// and an event already queued behind it. An empty queue resets the burst:
    /// the next `recv().await` is the cooperative yield.
    fn record(&mut self, work_remains: bool) -> bool {
        if !work_remains {
            self.consecutive_callbacks = 0;
            return false;
        }

        self.consecutive_callbacks += 1;
        if self.consecutive_callbacks < EVENT_CALLBACK_BUDGET {
            return false;
        }

        self.consecutive_callbacks = 0;
        true
    }

    fn reset(&mut self) {
        self.consecutive_callbacks = 0;
    }
}

/// The wire encoders are process-wide and outlive the batch they build, so
/// every suspension point in the dispatch path has to find them empty: a
/// half-filled encoder would leak its events into whatever batch is built after
/// the resumption, silently and without an error.
fn debug_assert_encoders_empty() {
    debug_assert!(MessageWireBatch::with_encoder(|encoder| encoder.is_empty()));
    debug_assert!(ReceiptWireBatch::with_encoder(|encoder| encoder.is_empty()));
    debug_assert!(ServerAckWireBatch::with_encoder(
        |encoder| encoder.is_empty()
    ));
}

/// Yield a macrotask so I/O callbacks (WebSocket, storage) can run.
async fn yield_to_io() {
    debug_assert_encoders_empty();
    crate::runtime::set_timeout_0().await;
}

/// Where a finished batch goes.
///
/// Without `onEventBatch` each batch crosses on its own, exactly as before.
/// With it, the batches an adjacent run of events produces are buffered and
/// cross together: a live message, its receipt and its ack cost one crossing
/// and one callback instead of three. Only what the dispatch loop already
/// pulled out of the channel is ever buffered, so nothing waits for anything.
struct BatchDelivery {
    envelope: Option<EventWireEnvelope>,
}

impl BatchDelivery {
    fn new(callbacks: &JsEventCallbacks) -> Self {
        Self {
            envelope: callbacks
                .on_event_batch
                .is_some()
                .then(EventWireEnvelope::default),
        }
    }

    /// Whether batches are buffered rather than delivered as they are built.
    fn is_open(&self) -> bool {
        self.envelope.as_ref().is_some_and(|e| !e.is_empty())
    }

    /// Events the next segment may carry. A segment is indivisible once built,
    /// so the split has to happen while the batch is still being filled: that
    /// is what keeps a whole envelope inside the boundary's event ceiling
    /// rather than one segment inside it.
    fn segment_capacity(&self) -> usize {
        match &self.envelope {
            Some(envelope) => EVENT_BATCH_CAPACITY
                .saturating_sub(envelope.records())
                .max(1),
            None => EVENT_BATCH_CAPACITY,
        }
    }

    /// Hand over the message encoder's current batch.
    async fn emit_message_batch(
        &mut self,
        callbacks: &JsEventCallbacks,
        budget: &mut EventDispatchBudget,
        work_remains: bool,
    ) {
        let Some(envelope) = self.envelope.as_mut() else {
            let encoded = MessageWireBatch::with_encoder(MessageWireBatch::finish);
            dispatch_message_wire_batch(callbacks, encoded, budget, work_remains).await;
            return;
        };
        MessageWireBatch::with_encoder(|encoder| {
            let records = encoder.len();
            envelope.push_segment(EVENT_SEGMENT_KIND_MESSAGE, records, |out| {
                encoder.write_and_reset(out)
            });
        });
        self.flush_if_full(callbacks, budget, work_remains).await;
    }

    /// Hand over a packed encoder's current batch.
    async fn emit_packed_batch<B: PackedBatchChannel>(
        &mut self,
        callbacks: &JsEventCallbacks,
        budget: &mut EventDispatchBudget,
        work_remains: bool,
    ) {
        let Some(envelope) = self.envelope.as_mut() else {
            let channel = B::channel(callbacks);
            match B::with_encoder(|encoder| encoder.finish(channel.buffer())) {
                Ok(batch) => {
                    if let Err(e) = channel.call(&callbacks.receiver, B::KIND, &batch) {
                        log::warn!("JS {} batch callback threw: {e:?}", B::KIND);
                    }
                }
                Err(e) => log::warn!("{} wire batch materialization failed: {e:?}", B::KIND),
            }
            if budget.record(work_remains) {
                yield_to_io().await;
            }
            return;
        };
        B::with_encoder(|encoder| {
            let records = encoder.len();
            envelope.push_segment(B::SEGMENT_KIND, records, |out| encoder.write_and_reset(out));
        });
        self.flush_if_full(callbacks, budget, work_remains).await;
    }

    /// Cross a run that has reached the ceiling, so the next segment starts
    /// from a full budget again.
    async fn flush_if_full(
        &mut self,
        callbacks: &JsEventCallbacks,
        budget: &mut EventDispatchBudget,
        work_remains: bool,
    ) {
        if self
            .envelope
            .as_ref()
            .is_some_and(|e| e.records() >= EVENT_BATCH_CAPACITY)
        {
            self.flush(callbacks, budget, work_remains).await;
        }
    }

    /// Cross whatever is buffered. Called before every suspension point and
    /// before any other callback, so buffering never reorders or delays a
    /// batch past something the host would otherwise have seen after it.
    ///
    /// A lone segment crosses through the callback and the buffer its own kind
    /// negotiated, so a run with nothing to coalesce is delivered exactly as it
    /// would have been without the envelope.
    async fn flush(
        &mut self,
        callbacks: &JsEventCallbacks,
        budget: &mut EventDispatchBudget,
        work_remains: bool,
    ) {
        let Some(envelope) = self.envelope.as_mut() else {
            return;
        };
        if envelope.is_empty() {
            return;
        }
        let lone = envelope
            .lone_segment_kind()
            .map(|kind| (kind, callbacks.channel_of_kind(kind)));
        let buffer = match lone {
            Some((_, Some((_, channel)))) => channel.buffer(),
            // A lone message segment, or an envelope: neither has a borrowing
            // form, because a message decode returns views over the buffer.
            _ => BatchBuffer::Owned,
        };
        let result = match envelope.finish(buffer) {
            CrossedBatch::Single { kind, batch } => match callbacks.channel_of_kind(kind) {
                Some((name, channel)) => channel.call(&callbacks.receiver, name, &batch),
                None => callbacks.call_message_batch(&batch),
            },
            CrossedBatch::Envelope(batch) => callbacks.call_event_batch(&batch),
        };
        if let Err(e) = result {
            log::warn!("JS batch callback threw: {e:?}");
        }
        if budget.record(work_remains) {
            yield_to_io().await;
        }
    }
}

/// Single consumer loop — guarantees event ordering.
/// Cooperatively yields only while a burst still has queued work.
async fn run_event_consumer(
    callbacks: &JsEventCallbacks,
    event_rx: async_channel::Receiver<Arc<Event>>,
) {
    let mut budget = EventDispatchBudget::default();
    let mut pending_event = None;
    let mut delivery = BatchDelivery::new(callbacks);
    loop {
        let event = match pending_event.take() {
            Some(event) => event,
            None => {
                // Nothing is left to coalesce: the dispatchers hand back their
                // cross-kind lookahead in `pending_event`, so `None` here means
                // their last `try_recv` found the channel empty. Cross before
                // parking rather than holding a run across an idle wait.
                delivery.flush(callbacks, &mut budget, false).await;
                debug_assert_encoders_empty();
                match event_rx.recv().await {
                    Ok(event) => {
                        record_history_event_dequeued(&event);
                        event
                    }
                    Err(_) => break,
                }
            }
        };
        dispatch_event_to_js(
            callbacks,
            event,
            &event_rx,
            &mut pending_event,
            &mut budget,
            &mut delivery,
        )
        .await;
    }
}

impl JsEventHandler {
    fn new(callbacks: JsEventCallbacks) -> Self {
        let (event_tx, event_rx) = async_channel::bounded::<Arc<Event>>(EVENT_CHANNEL_CAPACITY);

        wasm_bindgen_futures::spawn_local(async move {
            run_event_consumer(&callbacks, event_rx).await;
        });

        Self { event_tx }
    }

    fn enqueue(&self, event: Arc<Event>) {
        let history_compressed_bytes = history_event_compressed_bytes(&event);
        if let Some(compressed_bytes) = history_compressed_bytes {
            crate::memory_profile::record_history_event_enqueued(compressed_bytes);
        }
        if let Err(error) = self.event_tx.try_send(event) {
            if let Some(compressed_bytes) = history_compressed_bytes {
                crate::memory_profile::record_history_event_cancelled(compressed_bytes);
            }
            log::warn!("Event channel send failed: {error}");
        }
    }
}

/// Serialize + dispatch one event inside the consumer loop.
async fn dispatch_event_to_js(
    callbacks: &JsEventCallbacks,
    event: Arc<Event>,
    event_rx: &async_channel::Receiver<Arc<Event>>,
    pending_event: &mut Option<Arc<Event>>,
    budget: &mut EventDispatchBudget,
    delivery: &mut BatchDelivery,
) {
    // Anything that is not buffered into the open run has to see it delivered
    // first: the host observes batches in the order the events arrived, and a
    // buffered run must never trail a callback that came after it.
    if !callbacks.buffers_into_envelope(event.as_ref()) {
        delivery
            .flush(callbacks, budget, !event_rx.is_empty())
            .await;
    }

    // HistorySync is split into bounded batches so BOTH peaks are O(batch)
    // instead of O(chunk): the WASM decode peak (decoded protos) and the JS
    // heap peak (each batch's object tree dies before the next is built).
    // The host accumulates the batches (and gates `isLatest` on
    // the final batch). See dispatch_history_sync_batches.
    if let Event::HistorySync(lazy) = event.as_ref() {
        let compressed_bytes = lazy.compressed_bytes().len();
        // Wire batching is the only history-sync boundary: the legacy
        // structured-event path serialized every proto tree through the camel
        // serializer, which kept the whole waproto Serialize graph alive in
        // the binary even for hosts that never used it.
        if !callbacks.supports_history_sync_batching() {
            log::error!(
                "History sync dropped: the event callbacks must provide onHistorySyncBatch"
            );
        } else if let Err(e) = dispatch_history_sync_wire_batches(callbacks, lazy) {
            log::warn!("History sync stream failed: {e}");
        }
        budget.reset();
        // Release the compressed event before yielding to I/O. The next history
        // frame can then reuse its WASM pages, while V8 still gets one collection
        // window between chunks instead of one macrotask per tiny wire batch.
        drop(event);
        crate::memory_profile::record_history_event_completed(compressed_bytes);
        yield_to_io().await;
        return;
    }

    if let Event::Messages(_) = event.as_ref() {
        // Wire batching is the only message boundary: protobuf payloads cross
        // as bytes and the host decodes them with its own codec. The legacy
        // per-message JS-object path serialized the full Message tree through
        // the camel serializer, keeping that Serialize graph alive in the
        // binary for every host.
        if callbacks.supports_message_wire_batching() {
            dispatch_message_events(callbacks, event, event_rx, pending_event, budget, delivery)
                .await;
        } else {
            log::error!("Messages dropped: the event callbacks must provide onMessageBatch");
        }
        return;
    }
    // Optional packed fast paths: receipts and server acks arrive one per
    // stanza at message rate, so a host that opts in receives coalesced
    // typed-array batches instead of one reflected object per event. Hosts
    // without the callbacks keep the generic single-event path below.
    if ReceiptWireBatch::accepts(event.as_ref()) && callbacks.receipt_channel.is_registered() {
        dispatch_packed_events::<ReceiptWireBatch>(
            callbacks,
            event,
            event_rx,
            pending_event,
            budget,
            delivery,
        )
        .await;
        return;
    }
    if ServerAckWireBatch::accepts(event.as_ref()) && callbacks.server_ack_channel.is_registered() {
        dispatch_packed_events::<ServerAckWireBatch>(
            callbacks,
            event,
            event_rx,
            pending_event,
            budget,
            delivery,
        )
        .await;
        return;
    }

    match event_to_js(&event) {
        Ok(js_event) => dispatch_js_value(callbacks, js_event, budget, !event_rx.is_empty()).await,
        Err(e) => log::warn!("Event serialization failed: {e:?}"),
    }
}

/// JS delivery channel for a packed batch kind: which registered callback
/// receives it. Split from `PackedEventBatch` so the batch encoders in
/// `wire_batch` stay independent of the callbacks owner.
trait PackedBatchChannel: PackedEventBatch {
    fn channel(callbacks: &JsEventCallbacks) -> &PackedBatchChannelState;
}

impl PackedBatchChannel for ReceiptWireBatch {
    fn channel(callbacks: &JsEventCallbacks) -> &PackedBatchChannelState {
        &callbacks.receipt_channel
    }
}

impl PackedBatchChannel for ServerAckWireBatch {
    fn channel(callbacks: &JsEventCallbacks) -> &PackedBatchChannelState {
        &callbacks.server_ack_channel
    }
}

/// Coalesce an adjacent same-kind run into packed batches. The cross-kind
/// lookahead is retained in `pending_event`, preserving exact ordering.
///
/// Unlike the message path there is no collect turn and no minimum run: the
/// encoder's caches persist across batches, so a single event still costs two
/// bytes for each repeated address instead of a rebuilt object graph.
async fn dispatch_packed_events<B: PackedBatchChannel>(
    callbacks: &JsEventCallbacks,
    first_event: Arc<Event>,
    event_rx: &async_channel::Receiver<Arc<Event>>,
    pending_event: &mut Option<Arc<Event>>,
    budget: &mut EventDispatchBudget,
    delivery: &mut BatchDelivery,
) {
    let mut run = vec![first_event];
    while let Ok(next) = event_rx.try_recv() {
        if B::accepts(next.as_ref()) {
            run.push(next);
        } else {
            record_history_event_dequeued(&next);
            *pending_event = Some(next);
            break;
        }
    }

    let total = run.len();
    let mut start = 0;
    while start < total {
        let end = (start + delivery.segment_capacity()).min(total);
        // Encoding borrows the shared encoder; delivery happens after the
        // borrow ends so a re-entrant host cannot observe a half-built batch.
        B::with_encoder(|encoder| {
            encoder.begin();
            for event in &run[start..end] {
                if let Err(e) = encoder.push(event) {
                    log::warn!("{} wire serialization failed: {e:?}", B::KIND);
                }
            }
        });
        start = end;

        let work_remains = start < total || pending_event.is_some() || !event_rx.is_empty();
        delivery
            .emit_packed_batch::<B>(callbacks, budget, work_remains)
            .await;
    }
}

/// Coalesce adjacent message events that are already available in the ordered
/// channel. The bounded JS array caps transient V8/WASM allocation and the
/// non-message lookahead is retained in `pending_event`, preserving exact
/// cross-kind ordering. An isolated live singleton gets at most one explicit
/// I/O turn below; there is no duration-based batching timer.
async fn dispatch_message_events(
    callbacks: &JsEventCallbacks,
    first_event: Arc<Event>,
    event_rx: &async_channel::Receiver<Arc<Event>>,
    pending_event: &mut Option<Arc<Event>>,
    budget: &mut EventDispatchBudget,
    delivery: &mut BatchDelivery,
) {
    // A live message normally reaches this consumer as a singleton before the
    // next WebSocket callback has run, so a same-instant `try_recv` cannot see
    // useful batching work. When there is no backlog, yield exactly one I/O
    // turn (setImmediate on Node/Bun) and then drain what arrived. The typed
    // callback object is the explicit opt-in to this latency/throughput trade;
    // legacy function callbacks never take this path, and an existing core
    // batch/backlog never pays the extra yield.
    let is_singleton = matches!(
        first_event.as_ref(),
        Event::Messages(batch) if batch.len() == 1
    );
    // An open run is undelivered work, so the singleton's premise (nothing else
    // to hand over) does not hold and the extra turn would only delay it.
    if MESSAGE_SINGLETON_COLLECT_TURN && is_singleton && event_rx.is_empty() && !delivery.is_open()
    {
        yield_to_io().await;
    }

    let mut current_event = Some(first_event);
    // The encoder is process-wide and outlives this call, so a run that was
    // cancelled mid-batch could otherwise leak its messages into this one.
    MessageWireBatch::with_encoder(MessageWireBatch::reset);

    loop {
        let event = current_event
            .take()
            .expect("message dispatch always has a current event");
        let Event::Messages(batch) = event.as_ref() else {
            unreachable!("only message events enter the batching path");
        };

        for (index, inbound) in batch.iter().enumerate() {
            let capacity = delivery.segment_capacity();
            let full = MessageWireBatch::with_encoder(|encoder| {
                if let Err(e) = encoder.push(inbound) {
                    log::warn!("Message wire serialization failed: {e:?}");
                }
                encoder.len() >= capacity
            });
            if full {
                let more_in_core_batch = index + 1 < batch.len();
                delivery
                    .emit_message_batch(
                        callbacks,
                        budget,
                        more_in_core_batch || !event_rx.is_empty(),
                    )
                    .await;
            }
        }

        match event_rx.try_recv() {
            Ok(next) => {
                if matches!(next.as_ref(), Event::Messages(_)) {
                    current_event = Some(next);
                } else {
                    record_history_event_dequeued(&next);
                    *pending_event = Some(next);
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let work_remains = pending_event.is_some() || !event_rx.is_empty();
    if MessageWireBatch::with_encoder(|encoder| !encoder.is_empty()) {
        delivery
            .emit_message_batch(callbacks, budget, work_remains)
            .await;
    }
}

async fn dispatch_message_wire_batch(
    callbacks: &JsEventCallbacks,
    batch: Result<JsValue, JsValue>,
    budget: &mut EventDispatchBudget,
    work_remains: bool,
) {
    match batch {
        Ok(batch) => {
            if let Err(e) = callbacks.call_message_batch(&batch) {
                log::warn!("JS message batch callback threw: {e:?}");
            }
        }
        Err(e) => log::warn!("Message wire batch materialization failed: {e:?}"),
    }
    if budget.record(work_remains) {
        yield_to_io().await;
    }
}

async fn dispatch_js_value(
    callbacks: &JsEventCallbacks,
    js_event: JsValue,
    budget: &mut EventDispatchBudget,
    work_remains: bool,
) {
    if js_event.is_undefined() {
        return; // unhandled variant, already logged
    }
    if let Err(e) = callbacks.call_event(&js_event) {
        log::warn!("JS event callback threw: {:?}", e);
    }
    if budget.record(work_remains) {
        yield_to_io().await;
    }
}

#[cfg(test)]
mod event_dispatch_budget_tests {
    use super::{
        BatchBuffer, EVENT_CALLBACK_BUDGET, EVENT_CALLBACK_METHOD, EventDispatchBudget,
        HISTORY_SYNC_BATCH_CALLBACK_METHOD, HISTORY_SYNC_BATCH_MAX_BYTES,
        HISTORY_SYNC_BATCH_MAX_CONVERSATIONS, HISTORY_SYNC_CONVERSATION_TYPES_FIELD,
        JsEventCallbacks, MESSAGE_BATCH_CALLBACK_METHOD, RECEIPT_BATCH_BORROWED_CALLBACK_METHOD,
        RECEIPT_BATCH_CALLBACK_METHOD, history_sync_wire_batch_next_capacity,
        history_sync_wire_batch_should_flush,
    };
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn idle_queue_resets_the_burst_without_an_extra_yield() {
        let mut budget = EventDispatchBudget::default();
        for _ in 1..EVENT_CALLBACK_BUDGET {
            assert!(!budget.record(true));
        }

        assert!(!budget.record(false));
        for _ in 1..EVENT_CALLBACK_BUDGET {
            assert!(!budget.record(true));
        }
        assert!(budget.record(true));
    }

    #[test]
    fn sustained_backlog_yields_at_the_configured_budget() {
        let mut budget = EventDispatchBudget::default();
        for _ in 1..EVENT_CALLBACK_BUDGET {
            assert!(!budget.record(true));
        }
        assert!(budget.record(true));
        assert!(!budget.record(true));
    }

    #[test]
    fn wire_batches_are_bounded_by_bytes_but_never_split_one_conversation() {
        assert!(!history_sync_wire_batch_should_flush(
            0,
            0,
            HISTORY_SYNC_BATCH_MAX_BYTES + 1,
        ));
        assert!(!history_sync_wire_batch_should_flush(
            1,
            HISTORY_SYNC_BATCH_MAX_BYTES - 1,
            1,
        ));
        assert!(history_sync_wire_batch_should_flush(
            1,
            HISTORY_SYNC_BATCH_MAX_BYTES,
            1,
        ));
    }

    #[test]
    fn wire_batch_capacity_grows_geometrically_without_crossing_the_byte_budget() {
        let small = HISTORY_SYNC_BATCH_MAX_BYTES / 16;
        let initial_count =
            ((HISTORY_SYNC_BATCH_MAX_BYTES - 1) / small).min(HISTORY_SYNC_BATCH_MAX_CONVERSATIONS);
        let initial = small * initial_count;
        assert_eq!(history_sync_wire_batch_next_capacity(0, small), initial);
        assert_eq!(
            history_sync_wire_batch_next_capacity(initial, initial + 1),
            initial + 1,
        );

        let geometric = HISTORY_SYNC_BATCH_MAX_BYTES / 8;
        assert_eq!(
            history_sync_wire_batch_next_capacity(geometric, geometric + 1),
            geometric * 2,
        );

        let oversized = HISTORY_SYNC_BATCH_MAX_BYTES + 1;
        assert_eq!(
            history_sync_wire_batch_next_capacity(HISTORY_SYNC_BATCH_MAX_BYTES, oversized),
            oversized,
        );
    }

    #[test]
    fn accepts_the_legacy_function_callback() {
        let callback = js_sys::Function::new_no_args("");
        let parsed = JsEventCallbacks::from_js(callback.into()).expect("valid function");
        assert!(!parsed.supports_message_wire_batching());
        assert!(!parsed.supports_history_sync_batching());
        assert!(parsed.wants_history_sync_conversations(-1));
    }

    #[test]
    fn parses_the_message_wire_capability_once() {
        let callbacks = js_sys::Object::new();
        let single = js_sys::Function::new_no_args("");
        let messages = js_sys::Function::new_no_args("");
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(EVENT_CALLBACK_METHOD),
            &single,
        )
        .expect("set onEvent");
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(MESSAGE_BATCH_CALLBACK_METHOD),
            &messages,
        )
        .expect("set onMessageBatch");

        let parsed = JsEventCallbacks::from_js(callbacks.into()).expect("valid callback object");
        assert!(parsed.supports_message_wire_batching());
        assert!(!parsed.supports_history_sync_batching());
    }

    /// The borrow is negotiated by presence like every other batch capability,
    /// and only the borrowing form turns it on.
    #[test]
    fn parses_the_packed_batch_borrow_opt_in_once() {
        crate::wire_batch::reset_borrowed_batches();
        let build = |method: &str| {
            let callbacks = js_sys::Object::new();
            js_sys::Reflect::set(
                &callbacks,
                &JsValue::from_str(EVENT_CALLBACK_METHOD),
                &js_sys::Function::new_no_args(""),
            )
            .expect("set onEvent");
            js_sys::Reflect::set(
                &callbacks,
                &JsValue::from_str(method),
                &js_sys::Function::new_no_args(""),
            )
            .expect("set the receipt callback");
            JsEventCallbacks::from_js(callbacks.into()).expect("valid callback object")
        };

        let copying = build(RECEIPT_BATCH_CALLBACK_METHOD);
        assert!(copying.receipt_channel.is_registered());
        assert_eq!(copying.receipt_channel.buffer(), BatchBuffer::Owned);
        assert_eq!(copying.server_ack_channel.buffer(), BatchBuffer::Owned);

        let borrowing = build(RECEIPT_BATCH_BORROWED_CALLBACK_METHOD);
        assert!(borrowing.receipt_channel.is_registered());
        assert_eq!(borrowing.receipt_channel.buffer(), BatchBuffer::Borrowed);
        assert!(
            !borrowing.server_ack_channel.is_registered(),
            "opting one kind in enrolled the other"
        );
    }

    #[test]
    fn parses_the_history_sync_wire_capability_once() {
        let callbacks = js_sys::Object::new();
        let single = js_sys::Function::new_no_args("");
        let history = js_sys::Function::new_no_args("");
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(EVENT_CALLBACK_METHOD),
            &single,
        )
        .expect("set onEvent");
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(HISTORY_SYNC_BATCH_CALLBACK_METHOD),
            &history,
        )
        .expect("set onHistorySyncBatch");

        let parsed = JsEventCallbacks::from_js(callbacks.into()).expect("valid callback object");
        assert!(parsed.supports_history_sync_batching());
        assert!(parsed.wants_history_sync_conversations(5));
    }

    #[test]
    fn parses_history_sync_conversation_interest_once() {
        let callbacks = js_sys::Object::new();
        let single = js_sys::Function::new_no_args("");
        let types = js_sys::Array::new();
        types.push(&JsValue::from(0));
        types.push(&JsValue::from(3));
        types.push(&JsValue::from(6));
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(EVENT_CALLBACK_METHOD),
            &single,
        )
        .expect("set onEvent");
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(HISTORY_SYNC_CONVERSATION_TYPES_FIELD),
            &types,
        )
        .expect("set historySyncConversationTypes");

        let parsed = JsEventCallbacks::from_js(callbacks.into()).expect("valid callback object");
        for sync_type in [0, 3, 6] {
            assert!(parsed.wants_history_sync_conversations(sync_type));
        }
        for sync_type in [-1, 1, 2, 4, 5, 7] {
            assert!(!parsed.wants_history_sync_conversations(sync_type));
        }
    }

    #[test]
    fn rejects_a_non_function_batch_capability() {
        let callbacks = js_sys::Object::new();
        let single = js_sys::Function::new_no_args("");
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(EVENT_CALLBACK_METHOD),
            &single,
        )
        .expect("set onEvent");
        js_sys::Reflect::set(
            &callbacks,
            &JsValue::from_str(MESSAGE_BATCH_CALLBACK_METHOD),
            &JsValue::from_str("not-a-function"),
        )
        .expect("set invalid onMessageBatch");

        assert!(JsEventCallbacks::from_js(callbacks.into()).is_err());
    }
}

impl EventHandler for JsEventHandler {
    fn handle_event(&self, event: Arc<Event>) {
        self.enqueue(event);
    }
}

#[inline]
fn history_event_compressed_bytes(event: &Event) -> Option<usize> {
    match event {
        Event::HistorySync(lazy) => Some(lazy.compressed_bytes().len()),
        _ => None,
    }
}

#[inline]
fn record_history_event_dequeued(event: &Event) {
    if let Some(compressed_bytes) = history_event_compressed_bytes(event) {
        crate::memory_profile::record_history_event_dequeued(compressed_bytes);
    }
}

const HS_CONVERSATION_DATA_FIELD: &str = "conversationData";
const HS_CONVERSATION_OFFSETS_FIELD: &str = "conversationOffsets";
const HS_REMAINDER_DATA_FIELD: &str = "remainderData";
const HS_BATCH_INDEX_FIELD: &str = "batchIndex";
const HS_FINAL_BATCH_FIELD: &str = "isFinalBatch";

/// Preferred history-sync host boundary: the core performs its existing
/// bounded inflate/framing walk, while the host decodes each conversation wire
/// entry directly into its final object model. This removes the redundant
/// `wire -> owned Rust protobuf -> JS object` intermediate tree. Hosts that do
/// not advertise `onHistorySyncBatch` keep the legacy structured-event path.
fn dispatch_history_sync_wire_batches(
    callbacks: &JsEventCallbacks,
    lazy: &LazyHistorySync,
) -> Result<(), wacore::history_sync::HistorySyncError> {
    crate::memory_profile::record_history_sync(
        lazy.compressed_bytes().len(),
        lazy.decompressed_size(),
    );
    let mut stream = {
        let _scope = crate::memory_profile::enter_scope(
            crate::memory_profile::AllocationScope::HistoryDecode,
        );
        lazy.stream()
    };

    if !callbacks.wants_history_sync_conversations(lazy.sync_type()) {
        loop {
            let has_conversation = {
                let _scope = crate::memory_profile::enter_scope(
                    crate::memory_profile::AllocationScope::HistoryDecode,
                );
                stream.next_conversation_bytes()?.is_some()
            };
            if !has_conversation {
                break;
            }
            crate::memory_profile::record_history_conversation();
        }
        let remainder = {
            let _scope = crate::memory_profile::enter_scope(
                crate::memory_profile::AllocationScope::HistoryDecode,
            );
            stream.remainder()?
        };
        emit_hs_wire_batch(callbacks, lazy, &[], &[0], Some(&remainder), 0, true);
        return Ok(());
    }

    let mut batch_index = 0u32;
    let mut batch_len = 0usize;
    let mut conversation_data = Vec::new();
    let mut conversation_offsets = Vec::with_capacity(HISTORY_SYNC_BATCH_MAX_CONVERSATIONS + 1);
    conversation_offsets.push(0u32);

    loop {
        let wire_bytes = {
            let _scope = crate::memory_profile::enter_scope(
                crate::memory_profile::AllocationScope::HistoryDecode,
            );
            stream.next_conversation_bytes()?
        };
        let Some(wire_bytes) = wire_bytes else {
            break;
        };

        // Seeing one more entry proves the previous full batch was not final.
        // Emit before copying the borrowed entry: its bytes remain valid until
        // the next stream read, and the synchronous callback cannot retain a
        // reference into this reusable Rust buffer.
        if history_sync_wire_batch_should_flush(
            batch_len,
            conversation_data.len(),
            wire_bytes.len(),
        ) {
            emit_hs_wire_batch(
                callbacks,
                lazy,
                &conversation_data,
                &conversation_offsets,
                None,
                batch_index,
                false,
            );
            batch_index += 1;
            batch_len = 0;
            conversation_data.clear();
            conversation_offsets.clear();
            conversation_offsets.push(0);
        }

        crate::memory_profile::record_history_conversation();
        {
            let _scope = crate::memory_profile::enter_scope(
                crate::memory_profile::AllocationScope::HistorySerialize,
            );
            let end = conversation_data
                .len()
                .checked_add(wire_bytes.len())
                .and_then(|end| u32::try_from(end).ok())
                .ok_or_else(|| {
                    wacore::history_sync::HistorySyncError::MalformedProtobuf(
                        "history-sync wire batch exceeds Uint32 offset range".into(),
                    )
                })?;

            // `Vec::extend_from_slice` may geometrically double a nearly-full
            // one-page payload into a two-page allocation. Bound that policy
            // while retaining amortized growth for the smaller prefixes.
            let next_capacity =
                history_sync_wire_batch_next_capacity(conversation_data.capacity(), end as usize);
            if conversation_data.capacity() < next_capacity {
                conversation_data.reserve_exact(next_capacity - conversation_data.len());
            }
            conversation_data.extend_from_slice(wire_bytes);
            conversation_offsets.push(end);
        }
        batch_len += 1;
    }

    let remainder = {
        let _scope = crate::memory_profile::enter_scope(
            crate::memory_profile::AllocationScope::HistoryDecode,
        );
        stream.remainder()?
    };
    emit_hs_wire_batch(
        callbacks,
        lazy,
        &conversation_data,
        &conversation_offsets,
        Some(&remainder),
        batch_index,
        true,
    );
    Ok(())
}

fn emit_hs_wire_batch(
    callbacks: &JsEventCallbacks,
    lazy: &LazyHistorySync,
    conversation_data: &[u8],
    conversation_offsets: &[u32],
    remainder: Option<&waproto::whatsapp::HistorySync>,
    batch_index: u32,
    is_final: bool,
) {
    crate::memory_profile::record_history_batch();
    let batch = {
        let _scope = crate::memory_profile::enter_scope(
            crate::memory_profile::AllocationScope::HistoryEnvelope,
        );
        make_hs_wire_batch(
            lazy,
            conversation_data,
            conversation_offsets,
            remainder,
            batch_index,
            is_final,
        )
    };
    match batch {
        Ok(batch) => {
            let result = {
                let _scope = crate::memory_profile::enter_scope(
                    crate::memory_profile::AllocationScope::HistoryCallback,
                );
                callbacks.call_history_sync_batch(&batch)
            };
            match result {
                Ok(skipped) => {
                    if let Some(skipped) = skipped.as_f64()
                        && skipped.is_finite()
                        && skipped > 0.0
                    {
                        crate::memory_profile::record_history_skipped(
                            (skipped as usize).min(HISTORY_SYNC_BATCH_MAX_CONVERSATIONS),
                        );
                    }
                }
                Err(e) => log::warn!("JS history-sync batch callback threw: {e:?}"),
            }
        }
        Err(e) => log::warn!("History sync wire batch assembly failed: {e:?}"),
    }
}

fn history_sync_batch_data(
    lazy: &LazyHistorySync,
    remainder: Option<&waproto::whatsapp::HistorySync>,
    batch_index: u32,
    is_final: bool,
) -> Result<js_sys::Object, JsValue> {
    // LazyHistorySync owns the canonical notification metadata and its
    // Serialize implementation owns the field names. Keeping that contract in
    // whatsapp-rust means new core metadata automatically crosses the bridge
    // without another manually mirrored field list here.
    let data: js_sys::Object =
        crate::camel_serializer::to_js_value_camel_preserve_top_level_defaults(lazy)?
            .unchecked_into();

    // The non-conversation remainder crosses as one encoded protobuf payload
    // and the host decodes it with its own codec. Reflecting the tree into JS
    // here would keep the whole waproto Serialize graph compiled into the
    // binary just for this field.
    if let Some(remainder) = remainder {
        js_sys::Reflect::set(
            &data,
            &HS_REMAINDER_DATA_FIELD.into(),
            &js_sys::Uint8Array::from(waproto::codec::history_sync_to_vec(remainder).as_slice()),
        )?;
    }
    js_sys::Reflect::set(
        &data,
        &HS_BATCH_INDEX_FIELD.into(),
        &(batch_index as f64).into(),
    )?;
    js_sys::Reflect::set(&data, &HS_FINAL_BATCH_FIELD.into(), &is_final.into())?;
    Ok(data)
}

fn make_hs_wire_batch(
    lazy: &LazyHistorySync,
    conversation_data: &[u8],
    conversation_offsets: &[u32],
    remainder: Option<&waproto::whatsapp::HistorySync>,
    batch_index: u32,
    is_final: bool,
) -> Result<JsValue, JsValue> {
    let data = history_sync_batch_data(lazy, remainder, batch_index, is_final)?;
    js_sys::Reflect::set(
        &data,
        &HS_CONVERSATION_DATA_FIELD.into(),
        &js_sys::Uint8Array::from(conversation_data),
    )?;
    js_sys::Reflect::set(
        &data,
        &HS_CONVERSATION_OFFSETS_FIELD.into(),
        &js_sys::Uint32Array::from(conversation_offsets),
    )?;
    Ok(data.into())
}

/// Handles Event variants that need special serialization (no data, named
/// fields, multi-field payloads, or pre-processing).
fn event_to_js_special(event: &Event) -> Result<JsValue, JsValue> {
    let empty = || JsValue::from(js_sys::Object::new());

    let (event_type, data) = match event {
        Event::Connected(_) => ("connected", empty()),
        Event::Disconnected(_) => ("disconnected", empty()),
        Event::QrScannedWithoutMultidevice(_) => ("qr_scanned_without_multidevice", empty()),
        Event::ClientOutdated(_) => ("client_outdated", empty()),
        Event::StreamReplaced(_) => ("stream_replaced", empty()),
        Event::PairingQrCode(qr) => {
            let d = js_sys::Object::new();
            js_sys::Reflect::set(&d, &"code".into(), &qr.code.as_str().into())?;
            js_sys::Reflect::set(&d, &"timeout".into(), &(qr.timeout.as_secs() as f64).into())?;
            ("qr", d.into())
        }
        Event::PairingCode(pairing) => {
            let d = js_sys::Object::new();
            js_sys::Reflect::set(&d, &"code".into(), &pairing.code.as_str().into())?;
            js_sys::Reflect::set(
                &d,
                &"timeout".into(),
                &(pairing.timeout.as_secs() as f64).into(),
            )?;
            ("pairing_code", d.into())
        }
        Event::PairSuccess(ps) => {
            let d = js_sys::Object::new();
            js_sys::Reflect::set(&d, &"id".into(), &ps.id.to_string().into())?;
            js_sys::Reflect::set(&d, &"lid".into(), &ps.lid.to_string().into())?;
            js_sys::Reflect::set(
                &d,
                &"business_name".into(),
                &ps.business_name.as_str().into(),
            )?;
            js_sys::Reflect::set(&d, &"platform".into(), &ps.platform.as_str().into())?;
            ("pair_success", d.into())
        }
        Event::PairError(pe) => {
            let d = js_sys::Object::new();
            js_sys::Reflect::set(&d, &"id".into(), &pe.id.to_string().into())?;
            js_sys::Reflect::set(&d, &"lid".into(), &pe.lid.to_string().into())?;
            js_sys::Reflect::set(
                &d,
                &"business_name".into(),
                &pe.business_name.as_str().into(),
            )?;
            js_sys::Reflect::set(&d, &"platform".into(), &pe.platform.as_str().into())?;
            js_sys::Reflect::set(&d, &"error".into(), &pe.error.as_str().into())?;
            ("pair_error", d.into())
        }
        Event::LoggedOut(lo) => {
            let d = js_sys::Object::new();
            js_sys::Reflect::set(&d, &"on_connect".into(), &lo.on_connect.into())?;
            js_sys::Reflect::set(&d, &"reason".into(), &format!("{:?}", lo.reason).into())?;
            ("logged_out", d.into())
        }
        Event::Notification(node) => {
            let data = node_ref_to_js(node.get())?;
            ("notification", data)
        }
        Event::RawNode(node) => {
            let data = node_ref_to_js(node.get())?;
            ("raw_node", data)
        }
        // Messages and HistorySync never reach this function: the dispatch
        // loop intercepts them and requires the wire-batch callbacks, so they
        // fall through to the unhandled-variant log below if that invariant is
        // ever broken.
        other => {
            log::warn!(
                "unhandled event variant in event_to_js_special: {:?}",
                other
            );
            return Ok(JsValue::UNDEFINED);
        }
    };

    make_js_event(event_type, &data)
}

/// Parse `[major, minor, patch]` from a JS value into `(u32, u32, u32)`.
/// Returns `Ok(None)` if the value is null/undefined/missing.
fn parse_optional_version(
    value: Option<&JsValue>,
) -> Result<Option<(u32, u32, u32)>, crate::errors::BridgeError> {
    let Some(v) = value else { return Ok(None) };
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    if !js_sys::Array::is_array(v) {
        return Err(crate::errors::internal(
            "version must be a [major, minor, patch] array",
        ));
    }
    let arr = js_sys::Array::from(v);
    if arr.length() != 3 {
        return Err(crate::errors::internal(
            "version array must have exactly 3 elements [major, minor, patch]",
        ));
    }
    let parse = |i: u32| -> Result<u32, crate::errors::BridgeError> {
        let n = arr
            .get(i)
            .as_f64()
            .ok_or_else(|| crate::errors::internal("version array elements must be numbers"))?;
        if !n.is_finite() || n < 0.0 || n > u32::MAX as f64 || n.fract() != 0.0 {
            return Err(crate::errors::internal(
                "version array elements must be non-negative integers fitting in u32",
            ));
        }
        Ok(n as u32)
    };
    Ok(Some((parse(0)?, parse(1)?, parse(2)?)))
}

/// Parse the optional pre-key upload batch size. The core clamps to the
/// protocol-safe range at upload time, so we only reject inputs that can't be a
/// valid count here (non-numeric / negative / fractional / beyond u32).
fn parse_optional_count(
    value: Option<&JsValue>,
) -> Result<Option<usize>, crate::errors::BridgeError> {
    let Some(v) = value else { return Ok(None) };
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    let n = v
        .as_f64()
        .ok_or_else(|| crate::errors::internal("wantedPreKeyCount must be a number"))?;
    if !n.is_finite() || n < 0.0 || n > u32::MAX as f64 || n.fract() != 0.0 {
        return Err(crate::errors::internal(
            "wantedPreKeyCount must be a non-negative integer fitting in u32",
        ));
    }
    Ok(Some(n as usize))
}

/// Whether `value` is a millisecond count that survives the cast to `u64`.
///
/// The upper bound is not pedantry: `as u64` saturates, so a caller asking for
/// `2 ** 64` ms would otherwise get `u64::MAX` — around 584 million years —
/// instead of an error.
fn is_representable_millis(value: f64) -> bool {
    value.is_finite() && value >= 0.0 && value < u64::MAX as f64
}

const MILLIS_RANGE: &str = "must be a non-negative number of milliseconds below 2^64";

fn parse_optional_timeout_ms(
    field: &'static str,
    value: Option<f64>,
) -> Result<Option<std::time::Duration>, crate::errors::BridgeError> {
    match value {
        Some(value) if is_representable_millis(value) => {
            Ok(Some(std::time::Duration::from_millis(value as u64)))
        }
        Some(_) => Err(crate::errors::BridgeError::InvalidArgument {
            field: field.into(),
            reason: MILLIS_RANGE.into(),
        }),
        None => Ok(None),
    }
}

/// Same validation as [`parse_optional_timeout_ms`], for the calls where the
/// core takes a timeout rather than an option and there is no default to fall
/// back to.
fn parse_timeout_ms(
    field: &'static str,
    value: f64,
) -> Result<std::time::Duration, crate::errors::BridgeError> {
    if is_representable_millis(value) {
        Ok(std::time::Duration::from_millis(value as u64))
    } else {
        Err(crate::errors::BridgeError::InvalidArgument {
            field: field.into(),
            reason: MILLIS_RANGE.into(),
        })
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the WASM environment. Must be called once before creating clients.
///
/// Accepts an optional JS logger (pino-compatible) to route all Rust logs through.
/// If no logger is provided, falls back to console.log with "warn" level.
#[wasm_bindgen(js_name = initWasmEngine, skip_typescript)]
pub fn init_wasm_engine(logger: JsValue, crypto: JsValue) {
    console_error_panic_hook::set_once();

    #[cfg(feature = "memory-profiling")]
    if let Err(error) = crate::memory_profile::install_trace_profiler() {
        // Initialization is intentionally idempotent from JS. A second call
        // finds the same process-wide subscriber already installed.
        log::debug!("WASM allocation tracing subscriber already set: {error}");
    }

    if !logger.is_undefined() && !logger.is_null() {
        // Use the JS logger adapter — all Rust log::* calls go through pino
        let js_logger: crate::logger::JsLogger = logger.unchecked_into();
        let _ = crate::logger::set_logger(js_logger);
    } else {
        // No logger provided — fall back to console.log
        let _ = console_log::init_with_level(log::Level::Warn);
    }

    js_time::init_time_provider();

    if let Err(e) = crate::js_crypto::try_install_from_js(&crypto) {
        log::warn!("skipping native crypto provider: {e:?}");
    }
}

// ---------------------------------------------------------------------------
// Client creation
// ---------------------------------------------------------------------------

/// A full WhatsApp client running in WASM.
///
/// Usage from JS:
/// ```js
/// initWasmEngine();
/// const client = await createWhatsAppClient(transportConfig, httpConfig, onEvent);
/// await client.run();
/// ```
#[wasm_bindgen(js_name = createWhatsAppClient, skip_typescript)]
pub async fn create_whatsapp_client(
    transport_config: JsValue,
    http_config: JsValue,
    on_event: Option<JsValue>,
    store: Option<JsValue>,
    cache_config_js: Option<JsValue>,
    version_js: Option<JsValue>,
    wanted_pre_key_count_js: Option<JsValue>,
) -> Result<WasmWhatsAppClient, crate::errors::BridgeError> {
    // Block on every in-flight `Drop` cleanup before allocating new state.
    // Each `Drop` registers a oneshot; we await all of them. Closes the race
    // where a freshly constructed client shares the WASM heap with a previous
    // client's still-draining disconnect future.
    drain_drop_cleanups().await;

    let base_runtime = Arc::new(WasmRuntime) as Arc<dyn wacore::runtime::Runtime>;
    #[cfg(feature = "memory-profiling")]
    let (runtime, alloc_meter): (
        Arc<dyn wacore::runtime::Runtime>,
        Option<Arc<wacore::stats::AllocMeter>>,
    ) = {
        let meter = Arc::new(wacore::stats::AllocMeter::new());
        let instrument = Arc::new(crate::memory_profile::CoreTaskInstrument::new(
            meter.clone(),
        )) as Arc<dyn wacore::stats::TaskInstrument>;
        let metered_runtime = Arc::new(wacore::stats::InstrumentedRuntime::new(
            base_runtime,
            instrument,
        )) as Arc<dyn wacore::runtime::Runtime>;
        (
            Arc::new(crate::memory_profile::TraceContextRuntime::new(
                metered_runtime,
            )),
            Some(meter),
        )
    };
    #[cfg(not(feature = "memory-profiling"))]
    let (runtime, alloc_meter): (
        Arc<dyn wacore::runtime::Runtime>,
        Option<Arc<wacore::stats::AllocMeter>>,
    ) = (base_runtime, None);
    let backend: Arc<dyn wacore::store::traits::Backend> = match store {
        Some(ref store_val) if !store_val.is_null() && !store_val.is_undefined() => {
            let get_fn = js_sys::Reflect::get(store_val, &"get".into())
                .map_err(|_| crate::errors::internal("store.get is required"))?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| crate::errors::internal("store.get must be a function"))?;
            let set_fn = js_sys::Reflect::get(store_val, &"set".into())
                .map_err(|_| crate::errors::internal("store.set is required"))?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| crate::errors::internal("store.set must be a function"))?;
            let delete_fn = js_sys::Reflect::get(store_val, &"delete".into())
                .map_err(|_| crate::errors::internal("store.delete is required"))?
                .dyn_into::<js_sys::Function>()
                .map_err(|_| crate::errors::internal("store.delete must be a function"))?;
            // Optional batch primitives — feature-detected by handle presence.
            // A host that omits them keeps the per-key set/delete fallback.
            let opt_fn = |name: &str| {
                js_sys::Reflect::get(store_val, &name.into())
                    .ok()
                    .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
            };
            let set_many_fn = opt_fn("setMany");
            let delete_many_fn = opt_fn("deleteMany");
            let get_many_fn = opt_fn("getMany");
            let list_keys_fn = opt_fn("listKeys");
            let delete_prefix_fn = opt_fn("deletePrefix");
            // `capabilities` is read once: a declared capability must have its
            // method(s) present. Absent object => all false (legacy 3-method
            // behavior: the core keeps its self-maintained meta-indexes).
            let cap = |field: &str| {
                js_sys::Reflect::get(store_val, &"capabilities".into())
                    .ok()
                    .and_then(|c| js_sys::Reflect::get(&c, &field.into()).ok())
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            };
            let cap_batch = cap("batch");
            let cap_enumerate = cap("enumerate");
            let cap_prefix_delete = cap("prefixDelete");
            // Batch helpers are only honored when the host DECLARES `batch`, not
            // merely exposes the methods — keeps the contract (capability gates
            // behavior) and impl in lockstep. Drop the handles otherwise so the
            // backend falls back to per-key set/delete.
            let (set_many_fn, delete_many_fn, get_many_fn) = if cap_batch {
                (set_many_fn, delete_many_fn, get_many_fn)
            } else {
                (None, None, None)
            };
            info!(
                "Using JS-backed persistent storage (batch={}, enumerate={}, prefixDelete={})",
                cap_batch, cap_enumerate, cap_prefix_delete
            );
            js_backend::new_js_backend(js_backend::JsBackendHandles {
                get_fn,
                set_fn,
                delete_fn,
                set_many_fn,
                delete_many_fn,
                get_many_fn,
                list_keys_fn,
                delete_prefix_fn,
                cap_enumerate,
                cap_prefix_delete,
            })
        }
        _ => {
            info!("Using in-memory storage (no persistence)");
            js_backend::new_in_memory_backend()
        }
    };
    let transport_factory = Arc::new(JsTransportFactory::from_js(transport_config)?)
        as Arc<dyn wacore::net::TransportFactory>;
    let http_client =
        Arc::new(JsHttpClientAdapter::from_js(http_config)?) as Arc<dyn wacore::net::HttpClient>;

    let persistence_manager: Arc<whatsapp_rust::store::persistence_manager::PersistenceManager> =
        Arc::new(
            whatsapp_rust::store::persistence_manager::PersistenceManager::new(backend.clone())
                .await
                .map_err(|e| crate::errors::internal(format!("create persistence manager: {e}")))?,
        );

    let cache_config = build_cache_config(cache_config_js.as_ref())?;
    let override_version = parse_optional_version(version_js.as_ref())?;
    let wanted_pre_key_count = parse_optional_count(wanted_pre_key_count_js.as_ref())?;

    let (client, sync_rx) = whatsapp_rust::Client::new_with_cache_config(
        runtime.clone(),
        persistence_manager.clone(),
        transport_factory,
        http_client,
        override_version,
        cache_config,
    )
    .await;

    // Apply before connecting so it takes effect on the first pre-key upload;
    // smaller batches matter for the WASM/embedded heap (default is 812).
    if let Some(count) = wanted_pre_key_count {
        client.set_wanted_pre_key_count(count);
    }

    // Start the periodic saver AFTER the Client exists so we can subscribe to
    // its shutdown signal. The returned `AbortHandle` is stored on the wrapper
    // and also aborted explicitly in `disconnect()` — belt-and-suspenders with
    // the self-terminating shutdown arm in the saver loop.
    let saver_handle = persistence_manager.clone().run_background_saver(
        runtime.clone(),
        std::time::Duration::from_secs(5),
        client.shutdown_signal(),
    );

    let event_subscription = if let Some(callback) = on_event {
        let callbacks = JsEventCallbacks::from_js(callback)?;
        let handler = Arc::new(JsEventHandler::new(callbacks)) as Arc<dyn EventHandler>;
        Some(client.subscribe_handler(handler))
    } else {
        None
    };

    Ok(WasmWhatsAppClient {
        client,
        runtime,
        sync_rx: Some(sync_rx),
        saver_handle: Mutex::new(Some(saver_handle)),
        run_handle: Mutex::new(None),
        connection_handle: Mutex::new(None),
        sync_worker_handle: Mutex::new(None),
        _event_subscription: event_subscription,
        raw_node_lease: Mutex::new(None),
        alloc_meter,
    })
}

// ---------------------------------------------------------------------------
// Client wrapper
// ---------------------------------------------------------------------------

/// Opaque handle to the WhatsApp client.
#[wasm_bindgen]
pub struct WasmWhatsAppClient {
    client: Arc<whatsapp_rust::Client>,
    #[allow(dead_code)]
    runtime: Arc<dyn wacore::runtime::Runtime>,
    sync_rx: Option<async_channel::Receiver<whatsapp_rust::sync_task::MajorSyncTask>>,
    /// Handle to the bridge-owned background saver task. Aborted on
    /// `disconnect()` so the in-flight 5s `sleep` doesn't keep the Node.js
    /// event loop alive.
    saver_handle: Mutex<Option<wacore::runtime::AbortHandle>>,
    /// Spawned by `run()`; aborted on `Drop` so a `free()` without prior
    /// `disconnect()` doesn't leave the loop polling against the dropped
    /// wrapper.
    run_handle: Mutex<Option<wacore::runtime::AbortHandle>>,
    /// Spawned by `connect()` to read the single connection it established.
    /// Aborted on `Drop` for the same reason as `run_handle`.
    connection_handle: Mutex<Option<wacore::runtime::AbortHandle>>,
    sync_worker_handle: Mutex<Option<wacore::runtime::AbortHandle>>,
    /// Ownership token for the JS event sink. Dropping the wrapper removes the
    /// handler from the core event bus.
    _event_subscription: Option<wacore::types::events::Subscription>,
    /// At most one raw-node forwarding lease backs the boolean host API.
    raw_node_lease: Mutex<Option<whatsapp_rust::RawNodeLease>>,
    /// Core task allocation attribution; present only in diagnostics builds.
    alloc_meter: Option<Arc<wacore::stats::AllocMeter>>,
}

#[wasm_bindgen]
impl WasmWhatsAppClient {
    // ── Connection ───────────────────────────────────────────────────────

    /// Start the main client loop in the background.
    ///
    /// Spawns the connection loop (connect, handshake, message loop, reconnect)
    /// as a background task and returns immediately. The loop runs until `disconnect()`
    /// is called.
    ///
    /// Not `async` to avoid holding a wasm-bindgen borrow on `self` that would
    /// prevent calling other methods (disconnect, etc.).
    pub fn run(&mut self) -> Result<(), crate::errors::BridgeError> {
        if self.sync_rx.is_none() {
            return Err(crate::errors::internal("run() has already been called"));
        }
        let client = self.client.clone();
        let runtime = self.runtime.clone();
        let sync_rx = self.sync_rx.take();

        // Sync worker — processes history sync and app state sync tasks.
        // Must drain promptly to prevent the sync channel (capacity 32) from
        // blocking the message processing loop.
        if let Some(receiver) = sync_rx {
            let worker_client = client.clone();
            let handle = runtime.spawn(Box::pin(async move {
                while let Ok(task) = receiver.recv().await {
                    worker_client.process_sync_task(task).await;
                }
                info!("Sync worker shutting down.");
            }));
            *self
                .sync_worker_handle
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(handle);
        }

        let handle = runtime.spawn(Box::pin(async move {
            client.run().await;
            info!("Client run loop exited.");
        }));
        *self.run_handle.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);

        Ok(())
    }

    /// Connect to WhatsApp servers (single connection, no auto-reconnect).
    ///
    /// Resolves once the handshake is done, then a background task reads the
    /// connection until it ends. The core hands `connect()` back a `Connection`
    /// that decodes nothing until it is driven, so without that reader no event
    /// would ever fire and every request would time out.
    pub async fn connect(&self) -> Result<(), crate::errors::BridgeError> {
        let client = self.client.clone();
        let (handshake_tx, handshake_rx) = async_channel::bounded(1);

        // Connecting and reading live in one task because `Connection` borrows
        // the client it came from: the borrow cannot outlive this call, so the
        // handshake result travels back over the channel instead.
        let handle = self.runtime.spawn(Box::pin(async move {
            match client.connect().await {
                Err(e) => {
                    let _ = handshake_tx.send(Err(e)).await;
                }
                Ok(connection) => {
                    let _ = handshake_tx.send(Ok(())).await;
                    let _ = connection.read_until_disconnected().await;
                    info!("Client connection reader exited.");
                }
            }
        }));

        let handshake = handshake_rx.recv().await.map_err(|_| {
            crate::errors::internal("connect task ended without reporting a handshake result")
        })?;

        match handshake {
            Ok(()) => {
                *self
                    .connection_handle
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(handle);
                Ok(())
            }
            // The task is already finished; keep the slot pointing at whatever
            // reader is still live rather than at this failed attempt.
            Err(e) => {
                handle.abort();
                Err(crate::errors::BridgeError::from(e))
            }
        }
    }

    /// Fetch the account's reachout-timelock state.
    ///
    /// Wraps the `WAWebMexFetchReachoutTimelockJobQuery` MEX persisted
    /// query (id sourced from `wacore::iq::mex_operations::fetch_reachout_timelock`)
    /// and returns the `xwa2_fetch_account_reachout_timelock` payload as a
    /// raw JSON object — typically:
    ///
    /// ```json
    /// { "is_active": true,
    ///   "time_enforcement_ends": "1734567890",
    ///   "enforcement_type": "BIZ_COMMERCE_VIOLATION_…" }
    /// ```
    ///
    /// Returns `null` when the server has no timelock for this account.
    /// Callers map snake_case → idiomatic shape themselves.
    #[wasm_bindgen(js_name = "fetchReachoutTimelock")]
    pub async fn fetch_reachout_timelock(&self) -> Result<JsValue, crate::errors::BridgeError> {
        let payload = self.client.mex().fetch_reachout_timelock().await?;
        serde_wasm_bindgen::to_value(&payload)
            .map_err(|e| crate::errors::internal(format!("serialize reachout payload: {e}")))
    }

    /// Disconnect the client and flush pending state to storage.
    pub async fn disconnect(&self) {
        self.client.disconnect().await;
        // Core disconnect owns the final persistence flush. Abort the bridge
        // background saver afterwards so its pending timer cannot keep the
        // host event loop alive.
        if let Some(handle) = self
            .saver_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        // Drop the spawn-side `Abortable` wrappers so any straggler JsFuture
        // (e.g. a setImmediate yield about to fire) is released on next poll.
        if let Some(handle) = self
            .run_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        if let Some(handle) = self
            .sync_worker_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
    }

    /// Logout from WhatsApp — deregisters this companion device and disconnects.
    ///
    /// Sends `remove-companion-device` IQ to the server (best-effort),
    /// then disconnects. Does NOT clear stored keys — the caller should
    /// delete the store to fully clear credentials.
    pub async fn logout(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.logout().await;
        if let Some(handle) = self
            .saver_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        if let Some(handle) = self
            .run_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        if let Some(handle) = self
            .sync_worker_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            handle.abort();
        }
        Ok(())
    }

    /// Enable or disable automatic reconnection on disconnect.
    /// Enabled by default. When disabled, the client will not attempt
    /// to reconnect after an unexpected disconnection.
    #[wasm_bindgen(js_name = setAutoReconnect)]
    pub fn set_auto_reconnect(&self, enabled: bool) {
        self.client
            .enable_auto_reconnect
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Drop the current connection and reconnect immediately, picking up
    /// any profile changes (e.g. `setClientProfile`) on the new handshake.
    /// The `run()` loop continues — only the in-flight WebSocket is reset.
    #[wasm_bindgen(js_name = reconnect)]
    pub async fn reconnect(&self) {
        self.client.reconnect_immediately().await;
    }

    /// Check if the client is connected.
    #[wasm_bindgen(js_name = isConnected)]
    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    /// Check if the client is logged in (paired).
    #[wasm_bindgen(js_name = isLoggedIn)]
    pub fn is_logged_in(&self) -> bool {
        self.client.is_logged_in()
    }

    /// Wait until the socket is connected, or the timeout elapses.
    ///
    /// Resolves as soon as the socket is up — login may still be pending. The
    /// core rejects on expiry rather than reporting it, so this rejects with
    /// `kind === 'timeout'`.
    #[wasm_bindgen(js_name = waitForSocket)]
    pub async fn wait_for_socket(&self, timeout_ms: f64) -> Result<(), crate::errors::BridgeError> {
        let timeout = parse_timeout_ms("timeoutMs", timeout_ms)?;
        self.client
            .wait_for_socket(timeout)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Wait until the client is connected and logged in, or the timeout elapses.
    ///
    /// Stricter than `waitForSocket`: the core resolves this only once the
    /// connection is fully ready. Rejects with `kind === 'timeout'` on expiry.
    #[wasm_bindgen(js_name = waitForConnected)]
    pub async fn wait_for_connected(
        &self,
        timeout_ms: f64,
    ) -> Result<(), crate::errors::BridgeError> {
        let timeout = parse_timeout_ms("timeoutMs", timeout_ms)?;
        self.client
            .wait_for_connected(timeout)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Acknowledge a server dirty bit so the server stops re-announcing it.
    ///
    /// `dirtyType` is the wire string (`account_sync`, `groups`,
    /// `syncd_app_state`, `newsletter_metadata`); an unrecognized one is
    /// forwarded as-is, matching the core's fallback.
    #[wasm_bindgen(js_name = cleanDirtyBits)]
    pub async fn clean_dirty_bits(
        &self,
        dirty_type: &str,
        timestamp: Option<f64>,
    ) -> Result<(), crate::errors::BridgeError> {
        use wacore::iq::dirty::{DirtyBit, DirtyType};

        if dirty_type.is_empty() {
            return Err(crate::errors::BridgeError::InvalidArgument {
                field: "dirtyType".into(),
                reason: "must not be empty".into(),
            });
        }

        let kind = DirtyType::from(dirty_type);
        let bit = match timestamp {
            Some(value) if is_representable_millis(value) => {
                DirtyBit::with_timestamp(kind, value as u64)
            }
            Some(_) => {
                return Err(crate::errors::BridgeError::InvalidArgument {
                    field: "timestamp".into(),
                    reason: MILLIS_RANGE.into(),
                });
            }
            None => DirtyBit::new(kind),
        };

        self.client
            .clean_dirty_bits(bit)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Fetch the bot directory the server offers this account.
    ///
    /// There is no server push for it, so a caller that wants fresh data calls
    /// this again.
    #[wasm_bindgen(js_name = getBotList)]
    pub async fn get_bot_list(
        &self,
    ) -> Result<crate::result_types::BotListResult, crate::errors::BridgeError> {
        let list = self.client.bots().list().await?;
        Ok(bot_list_to_result(&list))
    }

    /// Fetch how many new chats this account may still start this cycle.
    #[wasm_bindgen(js_name = fetchNewChatMessageCappingInfo)]
    pub async fn fetch_new_chat_message_capping_info(
        &self,
    ) -> Result<crate::result_types::NewChatMessageCappingResult, crate::errors::BridgeError> {
        let capping = self
            .client
            .mex()
            .fetch_new_chat_message_capping_info()
            .await?;
        Ok(crate::result_types::NewChatMessageCappingResult {
            capping_status: capping
                .capping_status
                .as_ref()
                .map(|s| s.as_str().to_owned()),
            ote_status: capping.ote_status.as_ref().map(|s| s.as_str().to_owned()),
            mv_status: capping.mv_status.as_ref().map(|s| s.as_str().to_owned()),
            total_quota: capping.total_quota.map(|v| v as f64),
            used_quota: capping.used_quota.map(|v| v as f64),
            cycle_start_timestamp: capping.cycle_start_timestamp.map(|v| v as f64),
            cycle_end_timestamp: capping.cycle_end_timestamp.map(|v| v as f64),
            server_sent_timestamp: capping.server_sent_timestamp.map(|v| v as f64),
            remaining_quota: capping.remaining_quota().map(|v| v as f64),
        })
    }

    // ── Device props ─────────────────────────────────────────────────────

    /// Persist a push name before the first connection handshake.
    ///
    /// This is the WASM equivalent of whatsapp-rust's
    /// `BotBuilder::with_push_name`: it only forwards the value into the
    /// core device state and adds no bridge-specific protocol behavior.
    #[wasm_bindgen(js_name = setInitialPushName)]
    pub async fn set_initial_push_name(&self, name: String) {
        self.client
            .persistence_manager()
            .process_command(whatsapp_rust::wacore::store::DeviceCommand::SetPushName(
                name,
            ))
            .await;
    }

    /// Override `DeviceProps` before initial pairing. Only takes effect on
    /// the registration node — for already paired sessions this is a no-op
    /// on the wire and the core logs a warning.
    ///
    /// Setting `platformType: 'ANDROID_PHONE'` flips the phone's "Linked
    /// Devices" display to Android and unlocks server-side feature gating
    /// (e.g. view-once delivered as payload instead of `absent` stub) WITHOUT
    /// switching the underlying transport — the client still speaks the web
    /// protocol. Real Android companion mode (CRSC v2/v3, TEE attestation)
    /// is NOT implemented; if the server starts enforcing companion-type
    /// crypto, those connections may break.
    #[wasm_bindgen(js_name = setDeviceProps)]
    pub async fn set_device_props(&self, input: crate::device_props::DevicePropsInput) {
        self.client.set_device_props(input.into()).await;
    }

    /// Override the noise-handshake `ClientPayload` profile (UserAgent
    /// platform/device/os_version/manufacturer + `web_info` presence).
    ///
    /// Independent of `setDeviceProps`: that one drives the "Linked Devices"
    /// display on the phone; this one drives what the server sees during the
    /// noise handshake. Use `{ preset: 'android', osVersion: '13' }` to set
    /// `UserAgent.platform = ANDROID` and omit `web_info`.
    ///
    /// Runtime-only — the field is `#[serde(skip)]` in the persisted Device,
    /// so re-apply on every fresh process before `connect()`.
    #[wasm_bindgen(js_name = setClientProfile)]
    pub async fn set_client_profile(&self, input: crate::client_profile::ClientProfileInput) {
        self.client.set_client_profile(input.into()).await;
    }

    // ── Pairing ──────────────────────────────────────────────────────────

    /// Request a pairing code for phone number login (alternative to QR).
    ///
    /// Returns the 8-character pairing code to enter on the phone. On error,
    /// throws a `WhatsAppError` with structured fields (`kind`, `serverCode`,
    /// `serverText`, etc.) — see `errors::BridgeError`.
    #[wasm_bindgen(js_name = requestPairingCode)]
    pub async fn request_pairing_code(
        &self,
        phone_number: &str,
        custom_code: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        use whatsapp_rust::pair_code::PairCodeOptions;
        let options = PairCodeOptions {
            phone_number: phone_number.to_string(),
            custom_code,
            ..Default::default()
        };
        let code = self.client.pair_with_code(options).await?;
        Ok(code)
    }

    // ── Sending messages ─────────────────────────────────────────────────

    /// Send an E2E encrypted message from protobuf bytes.
    /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
    #[wasm_bindgen(js_name = sendMessageBytes)]
    pub async fn send_message_bytes(
        &self,
        jid: &str,
        bytes: &[u8],
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        let result = self.client.send_message(to, msg).await?;
        Ok(result.message_id)
    }

    /// Low-level message relay from protobuf binary bytes.
    #[wasm_bindgen(js_name = relayMessageBytes)]
    pub async fn relay_message_bytes(
        &self,
        jid: &str,
        bytes: &[u8],
        message_id: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        let mut options = whatsapp_rust::SendOptions::default();
        if let Some(message_id) = message_id {
            options = options.with_message_id(message_id);
        }
        send_message_with_options(&self.client, to, msg, options).await
    }

    /// Send an E2E message with neutral core-owned controls.
    ///
    /// Child nodes are converted only at the boundary. Routing, cache policy,
    /// encryption and reserved-node validation remain owned by the core.
    #[wasm_bindgen(js_name = relayMessageBytesWithOptions)]
    pub async fn relay_message_bytes_with_options(
        &self,
        jid: &str,
        bytes: &[u8],
        message_id: Option<String>,
        extra_nodes: JsBinaryNodeArray,
        refresh_group_metadata: bool,
        refresh_devices: bool,
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        let mut options = whatsapp_rust::SendOptions::default()
            .with_extra_stanza_nodes(js_node_array_to_vec(extra_nodes)?)
            .with_group_metadata_freshness(freshness(refresh_group_metadata))
            .with_device_freshness(freshness(refresh_devices));
        if let Some(message_id) = message_id {
            options = options.with_message_id(message_id);
        }
        send_message_with_options(&self.client, to, msg, options).await
    }

    /// Retransmit an existing message to one requesting device.
    #[wasm_bindgen(js_name = retransmitMessageBytes)]
    pub async fn retransmit_message_bytes(
        &self,
        chat_jid: &str,
        bytes: &[u8],
        input: crate::result_types::MessageRetransmissionInput,
    ) -> Result<(), crate::errors::BridgeError> {
        let (chat, msg) = parse_jid_and_msg_bytes(chat_jid, bytes)?;
        let requester = parse_jid(&input.requester_jid)?;
        let retry_count = u8::try_from(input.retry_count)
            .map_err(|_| crate::errors::internal("retry count exceeds the u8 range"))?;
        let mut request = whatsapp_rust::MessageRetransmission::new(
            chat,
            requester,
            msg,
            input.message_id,
            retry_count,
        )
        .with_group_metadata_freshness(freshness(input.refresh_group_metadata));
        if let Some(recipient_jid) = input.recipient_jid {
            request = request.with_recipient(parse_jid(&recipient_jid)?);
        }
        self.client.retransmit_message(request).await?;
        Ok(())
    }

    // ── Message management ──────────────────────────────────────────────

    /// Edit a previously sent message from protobuf bytes.
    #[wasm_bindgen(js_name = editMessageBytes)]
    pub async fn edit_message_bytes(
        &self,
        jid: &str,
        message_id: &str,
        bytes: &[u8],
    ) -> Result<String, crate::errors::BridgeError> {
        let (to, msg) = parse_jid_and_msg_bytes(jid, bytes)?;
        self.client
            .edit_message(to, message_id, msg)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Revoke (delete) a sent message.
    #[wasm_bindgen(js_name = revokeMessage)]
    pub async fn revoke_message(
        &self,
        jid: &str,
        message_id: &str,
        participant: Option<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let to = parse_jid(jid)?;

        let revoke_type = match participant {
            Some(p) => {
                let sender = parse_jid(&p)?;
                whatsapp_rust::RevokeType::Admin {
                    original_sender: sender,
                }
            }
            None => whatsapp_rust::RevokeType::Sender,
        };

        self.client
            .revoke_message(to, message_id, revoke_type)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Groups ───────────────────────────────────────────────────────────

    /// Get metadata for a group.
    #[wasm_bindgen(js_name = getGroupMetadata)]
    pub async fn group_metadata(
        &self,
        jid: &str,
    ) -> Result<crate::result_types::GroupMetadataResult, crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        let metadata = self.client.groups().get_metadata(&group_jid).await?;

        Ok(group_metadata_to_result(&metadata))
    }

    /// Create a new group.
    ///
    /// Returns the full `GroupMetadataResult` parsed directly from the
    /// server's create response — no follow-up `getGroupMetadata` IQ
    /// needed. Mirrors the reference client: the create reply
    /// already carries the complete `<group>` node (id, subject,
    /// creation, creator, participants, …).
    #[wasm_bindgen(js_name = createGroup)]
    pub async fn group_create(
        &self,
        subject: &str,
        participants: Vec<String>,
    ) -> Result<crate::result_types::GroupMetadataResult, crate::errors::BridgeError> {
        use whatsapp_rust::features::GroupParticipantOptions;

        let participant_options: Vec<GroupParticipantOptions> = participants
            .iter()
            .map(|p| parse_jid(p).map(GroupParticipantOptions::new))
            .collect::<Result<_, _>>()?;

        let options = whatsapp_rust::features::GroupCreateOptions::new(subject)
            .with_participants(participant_options);

        let result = self.client.groups().create_group(options).await?;

        Ok(group_metadata_to_result(&result.metadata))
    }

    /// Update a group's subject (name).
    #[wasm_bindgen(js_name = groupUpdateSubject)]
    pub async fn group_update_subject(
        &self,
        jid: &str,
        subject: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        let group_subject = whatsapp_rust::features::GroupSubject::new(subject)?;

        self.client
            .groups()
            .set_subject(&group_jid, group_subject)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Update a group's description. Pass null/undefined to remove.
    #[wasm_bindgen(js_name = groupUpdateDescription)]
    pub async fn group_update_description(
        &self,
        jid: &str,
        description: Option<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        let desc = description
            .as_deref()
            .map(whatsapp_rust::features::GroupDescription::new)
            .transpose()?;

        self.client
            .groups()
            // The caller holds no description id, so the core reads the current
            // one before sending: without that token the server rejects every
            // update of a group that already has a description.
            .set_description(
                &group_jid,
                desc,
                whatsapp_rust::features::PreviousDescription::Resolve,
            )
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Leave a group.
    #[wasm_bindgen(js_name = groupLeave)]
    pub async fn group_leave(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        self.client
            .groups()
            .leave(&group_jid)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Set or clear the bot's per-group "member label" — the small tag rendered
    /// under the bot's display name inside that group's UI. Empty `label`
    /// clears the label. The core sends this as a `ProtocolMessage` over the
    /// normal message path (not an IQ), matching WA Web's behavior.
    #[wasm_bindgen(js_name = updateMemberLabel)]
    pub async fn update_member_label(
        &self,
        group_jid: &str,
        label: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let parsed = parse_jid(group_jid)?;
        self.client
            .groups()
            .update_member_label_with_id(&parsed, label)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Update group participants.
    #[wasm_bindgen(js_name = groupParticipantsUpdate)]
    pub async fn group_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        action: crate::result_types::GroupParticipantAction,
    ) -> Result<Vec<crate::result_types::ParticipantChangeResult>, crate::errors::BridgeError> {
        participants_update(&self.client, jid, participants, action, false).await
    }

    /// Fetch all groups the user is participating in.
    #[wasm_bindgen(js_name = groupFetchAllParticipating, skip_typescript)]
    pub async fn group_fetch_all_participating(
        &self,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let groups = self.client.groups().get_participating().await?;

        let obj = js_sys::Object::new();
        for (key, metadata) in &groups {
            let result = group_metadata_to_result(metadata);
            let js_metadata = serde_wasm_bindgen::to_value(&result)?;
            // #767: get_participating now keys by Jid (was String) — stringify for the JS object key.
            js_sys::Reflect::set(&obj, &JsValue::from_str(&key.to_string()), &js_metadata)?;
        }
        Ok(obj.into())
    }

    /// Get the invite link for a group.
    #[wasm_bindgen(js_name = groupInviteCode)]
    pub async fn group_invite_code(&self, jid: &str) -> Result<String, crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;

        self.client
            .groups()
            .get_invite_link(&group_jid, false)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Update a group setting (locked, announce, membership_approval).
    #[wasm_bindgen(js_name = groupSettingUpdate)]
    pub async fn group_setting_update(
        &self,
        jid: &str,
        setting: crate::result_types::GroupSetting,
        value: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        use crate::result_types::GroupSetting;
        let group_jid = parse_jid(jid)?;

        match setting {
            GroupSetting::Locked => self.client.groups().set_locked(&group_jid, value).await?,
            GroupSetting::Announce => self.client.groups().set_announce(&group_jid, value).await?,
            GroupSetting::MembershipApproval => {
                let mode = if value {
                    whatsapp_rust::MembershipApprovalMode::On
                } else {
                    whatsapp_rust::MembershipApprovalMode::Off
                };
                self.client
                    .groups()
                    .set_membership_approval(&group_jid, mode)
                    .await?;
            }
        }

        Ok(())
    }

    /// Set disappearing messages timer for a group (0 to disable).
    #[wasm_bindgen(js_name = groupToggleEphemeral)]
    pub async fn group_toggle_ephemeral(
        &self,
        jid: &str,
        expiration: u32,
    ) -> Result<(), crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;
        self.client
            .groups()
            .set_ephemeral(&group_jid, expiration)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Revoke a group's invite link (generates new one).
    #[wasm_bindgen(js_name = groupRevokeInvite)]
    pub async fn group_revoke_invite(
        &self,
        jid: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;
        let new_code = self
            .client
            .groups()
            .get_invite_link(&group_jid, true)
            .await?;
        Ok(new_code)
    }

    // ── Parent groups ───────────────────────────────────────────────────

    /// Create a parent group with explicit protocol options.
    #[wasm_bindgen(js_name = createCommunity)]
    pub async fn create_community(
        &self,
        name: &str,
        description: Option<String>,
        closed: bool,
        allow_non_admin_sub_group_creation: bool,
        create_general_chat: bool,
    ) -> Result<crate::result_types::GroupMetadataResult, crate::errors::BridgeError> {
        let mut options = whatsapp_rust::features::CreateCommunityOptions::new(name);
        options.description = description;
        options.closed = closed;
        options.allow_non_admin_sub_group_creation = allow_non_admin_sub_group_creation;
        options.create_general_chat = create_general_chat;
        let result = self.client.community().create(options).await?;
        Ok(group_metadata_to_result(&result.metadata))
    }

    /// Create a subgroup already linked to a parent group.
    #[wasm_bindgen(js_name = createCommunitySubgroup)]
    pub async fn create_community_subgroup(
        &self,
        name: &str,
        participants: Vec<String>,
        parent_jid: &str,
    ) -> Result<crate::result_types::GroupMetadataResult, crate::errors::BridgeError> {
        let participants = participants
            .iter()
            .map(|participant| parse_jid(participant))
            .collect::<Result<Vec<_>, _>>()?;
        let parent = parse_jid(parent_jid)?;
        let result = self
            .client
            .community()
            .create_subgroup(name, &participants, parent)
            .await?;
        Ok(group_metadata_to_result(&result.metadata))
    }

    /// Deactivate a parent group without deleting its former subgroups.
    #[wasm_bindgen(js_name = deactivateCommunity)]
    pub async fn deactivate_community(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        self.client.community().deactivate(parse_jid(jid)?).await?;
        Ok(())
    }

    /// Link existing groups to a parent group.
    #[wasm_bindgen(js_name = linkCommunitySubgroups)]
    pub async fn link_community_subgroups(
        &self,
        parent_jid: &str,
        subgroup_jids: Vec<String>,
    ) -> Result<crate::result_types::CommunityLinkResult, crate::errors::BridgeError> {
        let parent = parse_jid(parent_jid)?;
        let subgroups = subgroup_jids
            .iter()
            .map(|subgroup| parse_jid(subgroup))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .client
            .community()
            .link_subgroups(parent, &subgroups)
            .await?;
        Ok(community_link_result(
            result.linked_jids,
            result.failed_groups,
        ))
    }

    /// Unlink groups from a parent group.
    #[wasm_bindgen(js_name = unlinkCommunitySubgroups)]
    pub async fn unlink_community_subgroups(
        &self,
        parent_jid: &str,
        subgroup_jids: Vec<String>,
        remove_orphan_members: bool,
    ) -> Result<crate::result_types::CommunityLinkResult, crate::errors::BridgeError> {
        let parent = parse_jid(parent_jid)?;
        let subgroups = subgroup_jids
            .iter()
            .map(|subgroup| parse_jid(subgroup))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .client
            .community()
            .unlink_subgroups(parent, &subgroups, remove_orphan_members)
            .await?;
        Ok(community_link_result(
            result.unlinked_jids,
            result.failed_groups,
        ))
    }

    /// Fetch subgroups of a parent group.
    #[wasm_bindgen(js_name = getCommunitySubgroups)]
    pub async fn get_community_subgroups(
        &self,
        parent_jid: &str,
    ) -> Result<Vec<crate::result_types::CommunitySubgroupResult>, crate::errors::BridgeError> {
        let parent = parse_jid(parent_jid)?;
        let groups = self.client.community().get_subgroups(&parent).await?;
        Ok(groups
            .into_iter()
            .map(|group| crate::result_types::CommunitySubgroupResult {
                id: group.id.to_string(),
                subject: group.subject,
                participant_count: group.participant_count.map(f64::from),
                creation: group.creation.map(|value| value as f64),
                owner: group.owner.map(|value| value.to_string()),
                is_default_sub_group: group.is_default_sub_group,
                is_general_chat: group.is_general_chat,
            })
            .collect())
    }

    /// Fetch all parent groups the account currently participates in.
    #[wasm_bindgen(js_name = communityFetchAllParticipating, skip_typescript)]
    pub async fn community_fetch_all_participating(
        &self,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let communities = self.client.community().get_participating().await?;
        let result = js_sys::Object::new();
        for (jid, metadata) in &communities {
            let value = serde_wasm_bindgen::to_value(&group_metadata_to_result(metadata))?;
            js_sys::Reflect::set(&result, &jid.to_string().into(), &value)?;
        }
        Ok(result.into())
    }

    /// Update participants of a parent group. Removing a participant also
    /// removes them from its linked groups.
    #[wasm_bindgen(js_name = communityParticipantsUpdate)]
    pub async fn community_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        action: crate::result_types::GroupParticipantAction,
    ) -> Result<Vec<crate::result_types::ParticipantChangeResult>, crate::errors::BridgeError> {
        participants_update(&self.client, jid, participants, action, true).await
    }

    // ── Contacts ─────────────────────────────────────────────────────────

    /// Check if one or more phone numbers / JIDs are registered on WhatsApp.
    ///
    /// Accepts either bare phone numbers (treated as PN JIDs) or full JIDs
    /// (`@s.whatsapp.net` for PN, `@lid` for LID). Mixed PN/LID inputs are
    /// transparently split into the two underlying usync queries by the core,
    /// so a single call is at most two IQs regardless of input size.
    ///
    /// Returns one `IsOnWhatsAppResult` per server hit — including the LID
    /// counterpart and business flag — eliminating the follow-up `fetchUserInfo`
    /// round trip the previous single-phone API forced callers into.
    #[wasm_bindgen(js_name = isOnWhatsApp)]
    pub async fn is_on_whatsapp(
        &self,
        phones: Vec<String>,
    ) -> Result<Vec<crate::result_types::IsOnWhatsAppResult>, crate::errors::BridgeError> {
        let jids: Vec<Jid> = phones
            .iter()
            .map(|p| {
                // Bare digits → PN JID; anything containing '@' → parse as full JID.
                if p.contains('@') {
                    parse_jid(p)
                } else {
                    Ok(Jid::pn(p))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        let results = self.client.contacts().is_on_whatsapp(&jids).await?;

        // Use `Jid::push_to` instead of `to_string()` — bypasses the
        // `fmt::Display` / `dyn Write` dispatch path the core ships a specialized
        // fast writer for. Each output `String` is still owned (required to cross
        // the WASM ABI), but we skip the formatter machinery and size the buffer
        // up front to avoid mid-push reallocations.
        fn jid_to_owned(jid: &Jid) -> String {
            let mut buf = String::with_capacity(jid.user.len() + jid.server.as_str().len() + 8);
            jid.push_to(&mut buf);
            buf
        }

        Ok(results
            .iter()
            .map(|r| crate::result_types::IsOnWhatsAppResult {
                jid: jid_to_owned(&r.jid),
                is_registered: r.is_registered,
                lid: r.lid.as_ref().map(jid_to_owned),
                pn_jid: r.pn_jid.as_ref().map(jid_to_owned),
                is_business: r.is_business,
                verified_name: r.verified_name.as_ref().and_then(|v| v.name.clone()),
            })
            .collect())
    }

    /// Get the profile picture URL for a user or group.
    ///
    /// `picture_type` should be "preview" or "image".
    #[wasm_bindgen(js_name = profilePictureUrl)]
    pub async fn profile_picture_url(
        &self,
        jid: &str,
        picture_type: crate::result_types::PictureType,
        timeout_ms: Option<f64>,
    ) -> Result<Option<crate::result_types::ProfilePictureInfo>, crate::errors::BridgeError> {
        use crate::result_types::PictureType;
        let target = parse_jid(jid)?;
        let preview = match picture_type {
            PictureType::Preview => true,
            PictureType::Image => false,
        };

        let result = self
            .client
            .contacts()
            .get_profile_picture_with_timeout(
                &target,
                preview,
                parse_optional_timeout_ms("timeoutMs", timeout_ms)?,
            )
            .await?;

        Ok(result.map(|pic| crate::result_types::ProfilePictureInfo {
            id: pic.id,
            url: pic.url,
            direct_path: pic.direct_path,
            hash: pic.hash,
        }))
    }

    /// Fetch user info for one or more JIDs.
    #[wasm_bindgen(js_name = fetchUserInfo, skip_typescript)]
    pub async fn fetch_user_info(
        &self,
        jids: Vec<String>,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed_jids: Vec<Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<Vec<_>, _>>()?;

        let result = self.client.contacts().get_user_info(&parsed_jids).await?;

        let obj = js_sys::Object::new();
        for (jid, info) in &result {
            let entry = crate::result_types::UserInfoResult {
                jid: info.jid.to_string(),
                lid: info.lid.as_ref().map(|l| l.to_string()),
                status: info.status.clone(),
                picture_id: info.picture_id.clone(),
                is_business: info.is_business,
                verified_name: info.verified_name.as_ref().and_then(|v| v.name.clone()),
                devices: info.devices.clone(),
            };
            let js_entry = serde_wasm_bindgen::to_value(&entry)?;
            js_sys::Reflect::set(&obj, &JsValue::from_str(&jid.to_string()), &js_entry)?;
        }
        Ok(obj.into())
    }

    // ── Profile ──────────────────────────────────────────────────────────

    /// Set the user's push name (display name).
    #[wasm_bindgen(js_name = setPushName)]
    pub async fn set_push_name(&self, name: &str) -> Result<(), crate::errors::BridgeError> {
        self.client
            .profile()
            .set_push_name(name)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Set the profile picture for the logged-in user.
    #[wasm_bindgen(js_name = updateProfilePicture)]
    pub async fn update_profile_picture(
        &self,
        img_data: Vec<u8>,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        let result = self.client.profile().set_profile_picture(img_data).await?;

        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Remove the profile picture for the logged-in user.
    #[wasm_bindgen(js_name = removeProfilePicture)]
    pub async fn remove_profile_picture(
        &self,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        let result = self.client.profile().remove_profile_picture().await?;

        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Set the profile picture for a group the user administers.
    ///
    /// Mirrors the core `SetProfilePictureSpec::set_group` path — same IQ as
    /// the self update, just routed at the JID level so admins can change a
    /// group's avatar from JS without an extra capability check.
    #[wasm_bindgen(js_name = setGroupProfilePicture)]
    pub async fn set_group_profile_picture(
        &self,
        group_jid: &str,
        img_data: Vec<u8>,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        use wacore_binary::JidExt;
        let target = parse_jid(group_jid)?;
        if !target.is_group() {
            return Err(crate::errors::internal(
                "setGroupProfilePicture: target jid must be a group jid",
            ));
        }
        let result = self
            .client
            .execute(wacore::iq::contacts::SetProfilePictureSpec::set_group(
                &target, img_data,
            ))
            .await?;
        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Remove a group's profile picture.
    #[wasm_bindgen(js_name = removeGroupProfilePicture)]
    pub async fn remove_group_profile_picture(
        &self,
        group_jid: &str,
    ) -> Result<crate::result_types::ProfilePictureResult, crate::errors::BridgeError> {
        use wacore_binary::JidExt;
        let target = parse_jid(group_jid)?;
        if !target.is_group() {
            return Err(crate::errors::internal(
                "removeGroupProfilePicture: target jid must be a group jid",
            ));
        }
        let result = self
            .client
            .execute(wacore::iq::contacts::SetProfilePictureSpec::remove_group(
                &target,
            ))
            .await?;
        Ok(crate::result_types::ProfilePictureResult { id: result.id })
    }

    /// Update the user's status text (about).
    #[wasm_bindgen(js_name = updateProfileStatus)]
    pub async fn update_profile_status(
        &self,
        status: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .profile()
            .set_status_text(status)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Blocking ──────────────────────────────────────────────────────────

    /// Block or unblock a contact.
    #[wasm_bindgen(js_name = updateBlockStatus)]
    pub async fn update_block_status(
        &self,
        jid: &str,
        action: crate::result_types::BlockAction,
    ) -> Result<(), crate::errors::BridgeError> {
        use crate::result_types::BlockAction;
        let target = parse_jid(jid)?;

        match action {
            BlockAction::Block => self.client.blocking().block(&target).await?,
            BlockAction::Unblock => self.client.blocking().unblock(&target).await?,
        }
        Ok(())
    }

    /// Fetch the full blocklist.
    #[wasm_bindgen(js_name = fetchBlocklist)]
    pub async fn fetch_blocklist(
        &self,
    ) -> Result<Vec<crate::result_types::BlocklistEntryResult>, crate::errors::BridgeError> {
        let entries = self.client.blocking().get_blocklist().await?;

        Ok(entries
            .iter()
            .map(|e| crate::result_types::BlocklistEntryResult {
                jid: e.jid.to_string(),
                timestamp: e.timestamp.map(|v| v as f64),
            })
            .collect())
    }

    // ── Chat actions ──────────────────────────────────────────────────────

    /// Pin or unpin a chat.
    #[wasm_bindgen(js_name = pinChat)]
    pub async fn pin_chat(&self, jid: &str, pin: bool) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        if pin {
            self.client.chat_actions().pin_chat(&chat_jid).await
        } else {
            self.client.chat_actions().unpin_chat(&chat_jid).await
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// Mute or unmute a chat.
    ///
    /// Pass a positive timestamp (ms) to mute until that time, or null/undefined to unmute.
    #[wasm_bindgen(js_name = muteChat)]
    pub async fn mute_chat(
        &self,
        jid: &str,
        mute_until: Option<f64>,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        match mute_until {
            Some(ts) => {
                self.client
                    .chat_actions()
                    .mute_chat_until(&chat_jid, ts as i64)
                    .await
            }
            None => self.client.chat_actions().unmute_chat(&chat_jid).await,
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// Archive or unarchive a chat.
    #[wasm_bindgen(js_name = archiveChat)]
    pub async fn archive_chat(
        &self,
        jid: &str,
        archive: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        if archive {
            self.client
                .chat_actions()
                .archive_chat(&chat_jid, None)
                .await
        } else {
            self.client
                .chat_actions()
                .unarchive_chat(&chat_jid, None)
                .await
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// Save or rename a contact, syncing the name to the user's linked devices
    /// (a `contact` app-state mutation). `jid` must be a bare phone-number JID
    /// (the core rejects LID/group/device-specific JIDs).
    #[wasm_bindgen(js_name = saveContact)]
    pub async fn save_contact(
        &self,
        jid: &str,
        full_name: Option<String>,
        first_name: Option<String>,
        save_on_primary_addressbook: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let contact_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .save_contact(
                &contact_jid,
                full_name,
                first_name,
                save_on_primary_addressbook,
            )
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Star or unstar a message.
    #[wasm_bindgen(js_name = starMessage)]
    pub async fn star_message(
        &self,
        jid: &str,
        message_id: &str,
        star: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;

        if star {
            self.client
                .chat_actions()
                .star_message(&chat_jid, None, message_id, true)
                .await
        } else {
            self.client
                .chat_actions()
                .unstar_message(&chat_jid, None, message_id, true)
                .await
        }
        .map_err(crate::errors::BridgeError::from)
    }

    /// React to a DM, group, or status@broadcast message. Empty/null `emoji`
    /// removes a previous reaction. For group/status targets `key.participant`
    /// must carry the original sender (DMs don't need it; for your own message
    /// `fromMe: true` suffices). For a Community Announcement Group the core
    /// encrypts the reaction with the target's `messageSecret` and sends
    /// `enc_reaction_message` (WA Web `WAWebReactionEncryptMsgData`) — plaintext
    /// reactions are rejected there, so this path must be used instead of a
    /// JS-built `reactionMessage` proto. Returns the reaction's message id.
    #[wasm_bindgen(js_name = sendReaction)]
    pub async fn send_reaction(
        &self,
        jid: &str,
        key: crate::result_types::TargetMessageKey,
        emoji: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let chat = parse_jid(jid)?;
        let target_key = waproto::whatsapp::MessageKey {
            remote_jid: Some(chat.to_string()),
            from_me: Some(key.from_me),
            id: Some(key.id),
            participant: key.participant,
        };
        let result = self
            .client
            .send_reaction(chat, target_key, emoji.as_deref().unwrap_or(""))
            .await
            .map_err(crate::errors::BridgeError::from)?;
        Ok(result.message_id)
    }

    /// Comment on a channel (CAG) post. `bytes` is the encoded body `Message`
    /// proto (encoding belongs to JS, like `sendMessageBytes`); `parent_key`
    /// references the post: `participant` is the post author, or `fromMe: true`
    /// for your own post (the core then resolves your LID/PN as the author).
    /// Requires the parent's `messageSecret`, captured when the post was
    /// received — the core derives the addon key and sends the encrypted
    /// comment envelope. Returns the comment's message id.
    #[wasm_bindgen(js_name = sendCommentBytes)]
    pub async fn send_comment_bytes(
        &self,
        jid: &str,
        parent_key: crate::result_types::TargetMessageKey,
        bytes: &[u8],
    ) -> Result<String, crate::errors::BridgeError> {
        // Without either, the core would fall back to the chat JID as the
        // author and fail the secret lookup the slow way — reject up front.
        if parent_key.participant.is_none() && !parent_key.from_me {
            return Err(crate::errors::internal(
                "parent_key needs participant (the post author) or fromMe: true",
            ));
        }
        let (chat, body) = parse_jid_and_msg_bytes(jid, bytes)?;
        let key = waproto::whatsapp::MessageKey {
            remote_jid: Some(chat.to_string()),
            from_me: Some(parent_key.from_me),
            id: Some(parent_key.id),
            participant: parent_key.participant,
        };
        let result = self
            .client
            .comments()
            .send_message(chat, key, body)
            .await
            .map_err(crate::errors::BridgeError::from)?;
        Ok(result.message_id)
    }

    /// Mark a chat as read or unread via app state mutation.
    /// Different from readMessages (which sends read receipts).
    #[wasm_bindgen(js_name = markChatAsRead)]
    pub async fn mark_chat_as_read(
        &self,
        jid: &str,
        read: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .mark_chat_as_read(&chat_jid, read, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a chat via app state mutation.
    #[wasm_bindgen(js_name = deleteChat)]
    pub async fn delete_chat(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .delete_chat(&chat_jid, true, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Clear a chat's messages while keeping the chat (WA Web's clearChat), via an
    /// app-state mutation. `delete_starred` also removes starred messages and
    /// `delete_media` also removes downloaded media (both flags live in the mutation
    /// index, not the proto). Mirrors `deleteChat` in passing `None` for the message
    /// range, i.e. clears the whole chat.
    #[wasm_bindgen(js_name = clearChat)]
    pub async fn clear_chat(
        &self,
        jid: &str,
        delete_starred: bool,
        delete_media: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .clear_chat(&chat_jid, delete_starred, delete_media, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a message for self (not for everyone).
    #[wasm_bindgen(js_name = deleteMessageForMe)]
    pub async fn delete_message_for_me(
        &self,
        jid: &str,
        message_id: &str,
        from_me: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .delete_message_for_me(&chat_jid, None, message_id, from_me, true, None)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Mute or unmute a contact's status updates.
    #[wasm_bindgen(js_name = setUserStatusMute)]
    pub async fn set_user_status_mute(
        &self,
        jid: &str,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let user_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .set_user_status_mute(&user_jid, muted)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Remove a contact from the address book on every linked device.
    ///
    /// Separate from `saveContact` rather than `saveContact(null)`: this is the
    /// one contact mutation the core sends as a syncd `Remove`, and a `Set`
    /// carrying an empty action would be applied as a rename to the empty
    /// string. `jid` must be a bare phone-number JID.
    #[wasm_bindgen(js_name = removeContact)]
    pub async fn remove_contact(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let contact_jid = parse_jid(jid)?;
        self.client
            .chat_actions()
            .remove_contact(&contact_jid)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Labels ────────────────────────────────────────────────────────────

    /// Create or rename a label. App state is an upsert keyed by `labelId`, so
    /// this both creates a label and edits an existing one. `color` is a
    /// WhatsApp color index.
    #[wasm_bindgen(js_name = createLabel)]
    pub async fn create_label(
        &self,
        label_id: &str,
        name: &str,
        color: i32,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .labels()
            .create_label(label_id, name, color)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a label.
    #[wasm_bindgen(js_name = deleteLabel)]
    pub async fn delete_label(&self, label_id: &str) -> Result<(), crate::errors::BridgeError> {
        self.client
            .labels()
            .delete_label(label_id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Associate a label with a chat.
    #[wasm_bindgen(js_name = addChatLabel)]
    pub async fn add_chat_label(
        &self,
        label_id: &str,
        chat_jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .add_chat_label(label_id, &chat)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Remove a label association from a chat.
    #[wasm_bindgen(js_name = removeChatLabel)]
    pub async fn remove_chat_label(
        &self,
        label_id: &str,
        chat_jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .remove_chat_label(label_id, &chat)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Associate a label with a single message.
    ///
    /// Keyed by the message as well as the chat, under a different action than
    /// the chat association. One message per call, mirroring the wire.
    #[wasm_bindgen(js_name = addMessageLabel)]
    pub async fn add_message_label(
        &self,
        label_id: &str,
        chat_jid: &str,
        message_id: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .add_message_label(label_id, &chat, message_id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Remove a label association from a single message.
    #[wasm_bindgen(js_name = removeMessageLabel)]
    pub async fn remove_message_label(
        &self,
        label_id: &str,
        chat_jid: &str,
        message_id: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        self.client
            .labels()
            .remove_message_label(label_id, &chat, message_id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Quick replies ─────────────────────────────────────────────────────

    /// Create or edit a quick reply. App state is an upsert keyed by `id`.
    ///
    /// `shortcut` is the `/`-typed trigger and `message` the expanded text;
    /// `keywords` are extra search terms and `count` the usage tally (`0` for a
    /// new one).
    #[wasm_bindgen(js_name = setQuickReply)]
    pub async fn set_quick_reply(
        &self,
        id: &str,
        shortcut: &str,
        message: &str,
        keywords: Vec<String>,
        count: i32,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .quick_replies()
            .set_quick_reply(id, shortcut, message, keywords, count)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete a quick reply.
    #[wasm_bindgen(js_name = deleteQuickReply)]
    pub async fn delete_quick_reply(&self, id: &str) -> Result<(), crate::errors::BridgeError> {
        self.client
            .quick_replies()
            .delete_quick_reply(id)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── App state settings ────────────────────────────────────────────────

    /// Turn outgoing link previews off or on for the whole account.
    ///
    /// The account's stored preference, replicated to the linked devices. It
    /// does not stop this client from attaching a preview it was explicitly
    /// asked to send.
    #[wasm_bindgen(js_name = setLinkPreviewsDisabled)]
    pub async fn set_link_previews_disabled(
        &self,
        disabled: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .app_state_settings()
            .set_link_previews_disabled(disabled)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Polls ─────────────────────────────────────────────────────────

    /// Create and send a poll. Returns `{ messageId, messageSecret }`.
    ///
    /// The `messageSecret` (32 bytes) is needed to decrypt votes later.
    #[wasm_bindgen(js_name = createPoll)]
    pub async fn create_poll(
        &self,
        jid: &str,
        name: &str,
        options: Vec<String>,
        selectable_count: u32,
    ) -> Result<crate::result_types::CreatePollResult, crate::errors::BridgeError> {
        let to = parse_jid(jid)?;
        let (result, message_secret) = self
            .client
            .polls()
            .create(&to, name, &options, selectable_count)
            .await?;
        Ok(crate::result_types::CreatePollResult {
            message_id: result.message_id,
            message_secret: message_secret.to_vec(),
        })
    }

    /// Vote on a poll. Returns message ID.
    #[wasm_bindgen(js_name = votePoll)]
    pub async fn vote_poll(
        &self,
        chat_jid: &str,
        poll_msg_id: &str,
        poll_creator_jid: &str,
        message_secret: &[u8],
        option_names: Vec<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        let creator = parse_jid(poll_creator_jid)?;
        let result = self
            .client
            .polls()
            .vote(&chat, poll_msg_id, &creator, message_secret, &option_names)
            .await?;
        Ok(result.message_id)
    }

    /// Send a status/story message to specified recipients.
    /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
    #[wasm_bindgen(js_name = sendStatusMessageBytes)]
    pub async fn send_status_message_bytes(
        &self,
        bytes: &[u8],
        recipients: Vec<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        send_status_message_with_options(
            &self.client,
            bytes,
            recipients,
            whatsapp_rust::StatusSendOptions::default(),
        )
        .await
    }

    /// Send a status message with a caller-provided ID, neutral child nodes and
    /// an explicit recipient-device freshness policy.
    #[wasm_bindgen(js_name = sendStatusMessageBytesWithOptions)]
    pub async fn send_status_message_bytes_with_options(
        &self,
        bytes: &[u8],
        recipients: Vec<String>,
        message_id: Option<String>,
        extra_nodes: JsBinaryNodeArray,
        refresh_devices: bool,
    ) -> Result<String, crate::errors::BridgeError> {
        let options = whatsapp_rust::StatusSendOptions {
            message_id,
            extra_stanza_nodes: js_node_array_to_vec(extra_nodes)?,
            device_freshness: freshness(refresh_devices),
            ..Default::default()
        };
        send_status_message_with_options(&self.client, bytes, recipients, options).await
    }

    // ── Read receipts ─────────────────────────────────────────────────

    /// Mark messages as read by sending read receipts.
    #[wasm_bindgen(js_name = readMessages)]
    pub async fn read_messages(
        &self,
        keys: Vec<crate::result_types::ReadMessageKey>,
    ) -> Result<(), crate::errors::BridgeError> {
        let mut grouped: HashMap<(String, Option<String>), Vec<String>> = HashMap::new();

        for key in keys {
            grouped
                .entry((key.remote_jid, key.participant))
                .or_default()
                .push(key.id);
        }

        for ((chat_jid_str, participant_str), ids) in grouped {
            let chat_jid = parse_jid(&chat_jid_str)?;
            let participant_jid = participant_str.as_deref().map(parse_jid).transpose()?;

            // #775: mark_as_read now takes &[&str] (alloc-aware); borrow the owned ids.
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.client
                .mark_as_read(&chat_jid, participant_jid.as_ref(), &id_refs)
                .await?;
        }

        Ok(())
    }

    /// Mark voice/video notes as played by sending played receipts
    /// (`<receipt type="played"|"played-self">`). Groups keys by chat +
    /// participant exactly like [`Self::read_messages`]; the core picks
    /// `played` vs `played-self` (newsletters) and sets `participant` only for
    /// group/broadcast chats, so the JS side just hands over the message keys.
    #[wasm_bindgen(js_name = markPlayed)]
    pub async fn mark_played(
        &self,
        keys: Vec<crate::result_types::ReadMessageKey>,
    ) -> Result<(), crate::errors::BridgeError> {
        let mut grouped: HashMap<(String, Option<String>), Vec<String>> = HashMap::new();

        for key in keys {
            grouped
                .entry((key.remote_jid, key.participant))
                .or_default()
                .push(key.id);
        }

        for ((chat_jid_str, participant_str), ids) in grouped {
            let chat_jid = parse_jid(&chat_jid_str)?;
            let participant_jid = participant_str.as_deref().map(parse_jid).transpose()?;

            // #775: mark_as_played now takes &[&str] (alloc-aware); borrow the owned ids.
            let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.client
                .mark_as_played(&chat_jid, participant_jid.as_ref(), &id_refs)
                .await?;
        }

        Ok(())
    }

    // ── Group invite ────────────────────────────────────────────────────

    /// Join a group using an invite code.
    #[wasm_bindgen(js_name = groupAcceptInvite)]
    pub async fn group_accept_invite(
        &self,
        code: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let jid = self.client.groups().join_with_invite_code(code).await?;
        Ok(jid.group_jid().to_string())
    }

    /// Join a group via a GroupInviteMessage (V4 invite).
    #[wasm_bindgen(js_name = groupAcceptInviteV4)]
    pub async fn group_accept_invite_v4(
        &self,
        group_jid: &str,
        code: &str,
        expiration: f64,
        admin_jid: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let admin = parse_jid(admin_jid)?;
        let result = self
            .client
            .groups()
            .join_with_invite_v4(&group, code, expiration as i64, &admin)
            .await?;
        Ok(result.group_jid().to_string())
    }

    /// Revoke invitation codes previously issued to participants.
    #[wasm_bindgen(js_name = groupRevokeInviteV4)]
    pub async fn group_revoke_invite_v4(
        &self,
        group_jid: &str,
        invited_jid: &str,
    ) -> Result<bool, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let invited = parse_jid(invited_jid)?;
        let responses = self
            .client
            .groups()
            .revoke_request_code(&group, &[invited])
            .await?;
        Ok(responses.iter().all(|response| response.is_ok()))
    }

    /// Get group info from an invite code (without joining).
    /// Returns the same shape as groupMetadata.
    #[wasm_bindgen(js_name = groupGetInviteInfo)]
    pub async fn group_get_invite_info(
        &self,
        code: &str,
    ) -> Result<crate::result_types::GroupMetadataResult, crate::errors::BridgeError> {
        let metadata = self.client.groups().get_invite_info(code).await?;
        Ok(group_metadata_to_result(&metadata))
    }

    /// Get list of pending join requests for a group.
    #[wasm_bindgen(js_name = groupRequestParticipantsList)]
    pub async fn group_request_participants_list(
        &self,
        jid: &str,
    ) -> Result<Vec<crate::result_types::MembershipRequestResult>, crate::errors::BridgeError> {
        let group_jid = parse_jid(jid)?;
        let list = self
            .client
            .groups()
            .get_membership_requests(&group_jid)
            .await?;
        Ok(list
            .iter()
            .map(|r| crate::result_types::MembershipRequestResult {
                jid: r.jid.to_string(),
                request_time: r.request_time.map(|t| t as f64),
            })
            .collect())
    }

    /// Approve or reject pending join requests.
    #[wasm_bindgen(js_name = groupRequestParticipantsUpdate)]
    pub async fn group_request_participants_update(
        &self,
        jid: &str,
        participants: Vec<String>,
        action: crate::result_types::GroupRequestAction,
    ) -> Result<Vec<crate::result_types::ParticipantChangeResult>, crate::errors::BridgeError> {
        use crate::result_types::GroupRequestAction;
        let group_jid = parse_jid(jid)?;
        let participant_jids: Vec<Jid> = participants
            .iter()
            .map(|s| parse_jid(s))
            .collect::<Result<Vec<_>, _>>()?;

        let responses = match action {
            GroupRequestAction::Approve => {
                self.client
                    .groups()
                    .approve_membership_requests(&group_jid, &participant_jids)
                    .await?
            }
            GroupRequestAction::Reject => {
                self.client
                    .groups()
                    .reject_membership_requests(&group_jid, &participant_jids)
                    .await?
            }
        };
        Ok(responses.iter().map(participant_change_to_result).collect())
    }

    // ── Privacy settings ──────────────────────────────────────────────

    /// Fetch all privacy settings.
    #[wasm_bindgen(js_name = fetchPrivacySettings)]
    pub async fn fetch_privacy_settings(&self) -> Result<JsValue, crate::errors::BridgeError> {
        let response = self.client.fetch_privacy_settings().await?;
        let map: std::collections::HashMap<&str, &str> = response
            .settings
            .iter()
            .map(|s| (s.category.as_str(), s.value.as_str()))
            .collect();
        serde_wasm_bindgen::to_value(&map).map_err(crate::errors::BridgeError::from)
    }

    /// Update a single privacy setting.
    #[wasm_bindgen(js_name = updatePrivacySetting)]
    pub async fn update_privacy_setting(
        &self,
        category: &str,
        value: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .set_privacy_setting(category.into(), value.into())
            .await?;
        Ok(())
    }

    /// Set default disappearing messages duration (seconds). 0 to disable.
    #[wasm_bindgen(js_name = updateDefaultDisappearingMode)]
    pub async fn update_default_disappearing_mode(
        &self,
        duration: u32,
    ) -> Result<(), crate::errors::BridgeError> {
        self.client
            .set_default_disappearing_mode(duration)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Calls ────────────────────────────────────────────────────────────

    /// Reject an incoming call.
    #[wasm_bindgen(js_name = rejectCall)]
    pub async fn reject_call(
        &self,
        call_id: &str,
        peer: &str,
        call_creator: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let peer = parse_jid(peer)?;
        let call_creator = parse_jid(call_creator)?;
        self.client
            .voip()
            .reject_call(call_id, &peer, &call_creator)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── User status ──────────────────────────────────────────────────────

    /// Fetch user status/about text for one or more JIDs.
    #[wasm_bindgen(js_name = fetchStatus)]
    pub async fn fetch_status(
        &self,
        jids: Vec<String>,
    ) -> Result<Vec<crate::result_types::FetchStatusResult>, crate::errors::BridgeError> {
        let parsed_jids: Vec<Jid> = jids
            .iter()
            .map(|s| parse_jid(s))
            .collect::<Result<_, _>>()?;
        let infos = self.client.contacts().get_user_info(&parsed_jids).await?;
        Ok(infos
            .values()
            .map(|info| crate::result_types::FetchStatusResult {
                jid: info.jid.to_string(),
                status: info.status.clone(),
            })
            .collect())
    }

    // ── Business profile ───────────────────────────────────────────────

    /// Get business profile information for a JID.
    #[wasm_bindgen(js_name = getBusinessProfile)]
    pub async fn get_business_profile(
        &self,
        jid: &str,
    ) -> Result<Option<crate::result_types::BusinessProfileResult>, crate::errors::BridgeError>
    {
        let target_jid = parse_jid(jid)?;
        let profile = self.client.get_business_profile(&target_jid).await?;
        Ok(profile.map(|p| business_profile_to_result(&p)))
    }

    // ── Message history ──────────────────────────────────────────────────

    /// Request on-demand message history from the primary phone.
    /// Returns the message ID of the PDO request.
    /// Results will arrive as history_sync events.
    #[wasm_bindgen(js_name = fetchMessageHistory)]
    pub async fn fetch_message_history(
        &self,
        count: i32,
        chat_jid: &str,
        oldest_msg_id: &str,
        oldest_msg_from_me: bool,
        oldest_msg_timestamp_ms: f64,
    ) -> Result<String, crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;
        let msg_id = self
            .client
            .fetch_message_history(
                &chat,
                oldest_msg_id,
                oldest_msg_from_me,
                oldest_msg_timestamp_ms as i64,
                count,
            )
            .await?;
        Ok(msg_id)
    }

    // ── Group member add mode ────────────────────────────────────────────

    /// Set who can add members to a group.
    #[wasm_bindgen(js_name = groupMemberAddMode)]
    pub async fn group_member_add_mode(
        &self,
        jid: &str,
        mode: crate::result_types::MemberAddMode,
    ) -> Result<(), crate::errors::BridgeError> {
        use crate::result_types::MemberAddMode;
        let group_jid = parse_jid(jid)?;
        let add_mode = match mode {
            MemberAddMode::AdminAdd => whatsapp_rust::features::MemberAddMode::AdminAdd,
            MemberAddMode::AllMemberAdd => whatsapp_rust::features::MemberAddMode::AllMemberAdd,
        };
        self.client
            .groups()
            .set_member_add_mode(&group_jid, add_mode)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Presence ─────────────────────────────────────────────────────────

    /// Send presence status ("available" or "unavailable").
    #[wasm_bindgen(js_name = sendPresence)]
    pub async fn send_presence(
        &self,
        status: crate::result_types::PresenceStatus,
    ) -> Result<(), crate::errors::BridgeError> {
        use crate::result_types::PresenceStatus;
        let presence_status = match status {
            PresenceStatus::Available => whatsapp_rust::features::PresenceStatus::Available,
            PresenceStatus::Unavailable => whatsapp_rust::features::PresenceStatus::Unavailable,
        };

        self.client
            .presence()
            .set(presence_status)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Subscribe to a contact's presence updates.
    #[wasm_bindgen(js_name = presenceSubscribe)]
    pub async fn presence_subscribe(&self, jid: &str) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;

        self.client
            .presence()
            .subscribe(&target)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Newsletter ────────────────────────────────────────────────────────

    /// Create a new newsletter (channel).
    #[wasm_bindgen(js_name = newsletterCreate)]
    pub async fn newsletter_create(
        &self,
        name: &str,
        description: Option<String>,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let result = self
            .client
            .newsletter()
            .create(name, description.as_deref())
            .await?;

        Ok(newsletter_metadata_to_result(&result))
    }

    /// Fetch metadata for a newsletter by JID.
    #[wasm_bindgen(js_name = newsletterMetadata)]
    pub async fn newsletter_metadata(
        &self,
        jid: &str,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;

        let result = self.client.newsletter().get_metadata(&target).await?;

        Ok(newsletter_metadata_to_result(&result))
    }

    /// Subscribe (join) a newsletter.
    #[wasm_bindgen(js_name = newsletterSubscribe)]
    pub async fn newsletter_subscribe(
        &self,
        jid: &str,
    ) -> Result<crate::result_types::NewsletterMetadataResult, crate::errors::BridgeError> {
        let target = parse_jid(jid)?;

        let result = self.client.newsletter().join(&target).await?;

        Ok(newsletter_metadata_to_result(&result))
    }

    /// Unsubscribe (leave) a newsletter.
    #[wasm_bindgen(js_name = newsletterUnsubscribe)]
    pub async fn newsletter_unsubscribe(
        &self,
        jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;

        self.client
            .newsletter()
            .leave(&target)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Send a reaction to a newsletter message.
    ///
    /// `server_id` is the server-assigned message ID (passed as string to avoid
    /// JS number precision issues). `reaction` is the emoji code, or null/empty
    /// to remove a reaction.
    #[wasm_bindgen(js_name = newsletterReactMessage)]
    pub async fn newsletter_react_message(
        &self,
        jid: &str,
        server_id: &str,
        reaction: Option<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        let sid: u64 = server_id
            .parse()
            .map_err(|e| crate::errors::internal(format!("invalid server_id: {e}")))?;
        self.client
            .newsletter()
            .send_reaction(&target, sid, reaction.as_deref().unwrap_or(""))
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Mute or unmute a newsletter's follower-activity notifications — the channel
    /// mute a subscriber toggles (WA Web `MUTE_FOLLOWER_ACTIVITY`). `muted = true`
    /// silences them. The separate owner-only admin-activity mute is not exposed.
    #[wasm_bindgen(js_name = newsletterMute)]
    pub async fn newsletter_mute(
        &self,
        jid: &str,
        muted: bool,
    ) -> Result<(), crate::errors::BridgeError> {
        let target = parse_jid(jid)?;
        self.client
            .newsletter()
            .set_follower_mute(&target, muted)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Media reupload ────────────────────────────────────────────────────

    /// Request the server to re-upload expired media.
    ///
    /// Returns the new `directPath` on success.
    /// Throws on failure (not found, decryption error, timeout, etc.).
    #[wasm_bindgen(js_name = requestMediaReupload)]
    pub async fn request_media_reupload(
        &self,
        msg_id: &str,
        chat_jid: &str,
        media_key: &[u8],
        is_from_me: bool,
        participant: Option<String>,
    ) -> Result<String, crate::errors::BridgeError> {
        let chat = parse_jid(chat_jid)?;

        let participant_jid = participant.as_deref().map(parse_jid).transpose()?;

        let req = whatsapp_rust::MediaReuploadRequest {
            msg_id,
            chat_jid: &chat,
            media_key,
            is_from_me,
            participant: participant_jid.as_ref(),
        };

        let result = self.client.media_reupload().request(&req).await?;

        match result {
            whatsapp_rust::MediaRetryResult::Success { direct_path } => Ok(direct_path),
            whatsapp_rust::MediaRetryResult::NotFound => {
                Err(crate::errors::internal("Media not found on server"))
            }
            whatsapp_rust::MediaRetryResult::DecryptionError => {
                Err(crate::errors::internal("Media decryption error"))
            }
            whatsapp_rust::MediaRetryResult::GeneralError => {
                Err(crate::errors::internal("Media reupload failed"))
            }
        }
    }

    // ── Chat state ───────────────────────────────────────────────────────

    /// Send a chat state update (typing indicator).
    #[wasm_bindgen(js_name = sendChatState)]
    pub async fn send_chat_state(
        &self,
        jid: &str,
        state: crate::result_types::ChatState,
    ) -> Result<(), crate::errors::BridgeError> {
        use crate::result_types::ChatState;
        let to = parse_jid(jid)?;

        let chat_state = match state {
            ChatState::Composing => whatsapp_rust::features::ChatStateType::Composing,
            ChatState::Recording => whatsapp_rust::features::ChatStateType::Recording,
            ChatState::Paused => whatsapp_rust::features::ChatStateType::Paused,
        };

        self.client
            .chatstate()
            .send(&to, chat_state)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    // ── Media ────────────────────────────────────────────────────────────

    /// Get media connection info (auth token + upload hosts).
    ///
    /// Returns `{ auth: string, ttl: number, hosts: [{hostname: string, maxContentLengthBytes: number}] }`.
    #[wasm_bindgen(js_name = getMediaConn)]
    pub async fn get_media_conn(
        &self,
        force: bool,
    ) -> Result<crate::result_types::MediaConnResult, crate::errors::BridgeError> {
        let conn = self.client.refresh_media_conn(force).await?;

        Ok(crate::result_types::MediaConnResult {
            auth: conn.auth.clone(),
            ttl: conn.ttl as f64,
            hosts: conn
                .hosts
                .iter()
                .map(|h| crate::result_types::MediaHost {
                    hostname: h.hostname.clone(),
                })
                .collect(),
        })
    }

    /// Download and decrypt media from raw parameters.
    ///
    /// Handles CDN failover, auth refresh, HMAC-SHA256 verification, and
    /// AES-256-CBC decryption internally. Returns decrypted media bytes.
    #[wasm_bindgen(js_name = downloadMedia)]
    pub async fn download_media(
        &self,
        direct_path: &str,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        media_type: crate::result_types::MediaType,
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let mt: wacore::download::MediaType = media_type.into();
        let data = self
            .client
            .download_from_params(&whatsapp_rust::download::DownloadParams::encrypted(
                direct_path,
                media_key,
                file_sha256,
                file_enc_sha256,
                file_length as u64,
                mt,
            ))
            .await?;
        Ok(js_sys::Uint8Array::from(&data[..]))
    }

    /// Download, decrypt, and return a Web ReadableStream of decrypted chunks.
    ///
    /// Same as `downloadMedia` but returns a `ReadableStream` instead of buffering
    /// the entire file. In Node.js, consume with `Readable.fromWeb(stream)`.
    #[wasm_bindgen(js_name = downloadMediaStream)]
    pub fn download_media_stream(
        &self,
        direct_path: &str,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        media_type: crate::result_types::MediaType,
    ) -> Result<web_sys::ReadableStream, crate::errors::BridgeError> {
        let mt: wacore::download::MediaType = media_type.into();
        let client = self.client.clone();
        let direct_path = direct_path.to_string();
        let media_key = media_key.to_vec();
        let file_sha256 = file_sha256.to_vec();
        let file_enc_sha256 = file_enc_sha256.to_vec();
        let file_length = file_length as u64;

        // Channel with backpressure (capacity 2 keeps memory bounded)
        let (mut tx, rx) = futures::channel::mpsc::channel::<Result<JsValue, JsValue>>(2);

        wasm_bindgen_futures::spawn_local(async move {
            use futures::SinkExt;

            match client
                .download_from_params(&whatsapp_rust::download::DownloadParams::encrypted(
                    direct_path.as_str(),
                    &media_key,
                    &file_sha256,
                    &file_enc_sha256,
                    file_length,
                    mt,
                ))
                .await
            {
                Ok(data) => {
                    // Stream in 64KB chunks to avoid holding the full buffer in JS
                    const CHUNK_SIZE: usize = 65536;
                    for chunk in data.chunks(CHUNK_SIZE) {
                        let js_chunk = js_sys::Uint8Array::from(chunk);
                        if tx.send(Ok(js_chunk.into())).await.is_err() {
                            break; // Consumer cancelled the stream
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(JsValue::from_str(&e.to_string()))).await;
                }
            }
            // tx dropped here → stream ends
        });

        let readable = wasm_streams::ReadableStream::from_stream(rx);
        Ok(readable.into_raw())
    }

    // ── Upload ────────────────────────────────────────────────────────────

    /// Upload media: encrypt in memory + upload with CDN failover and retry.
    ///
    /// Takes raw plaintext bytes. Handles AES-256-CBC encryption, HMAC-SHA256
    /// signing, multi-host CDN upload, auth refresh, and resumable upload (>=5MB).
    #[wasm_bindgen(js_name = uploadMedia)]
    pub async fn upload_media(
        &self,
        data: &[u8],
        media_type: crate::result_types::MediaType,
    ) -> Result<crate::result_types::UploadMediaResult, crate::errors::BridgeError> {
        let mt: wacore::download::MediaType = media_type.into();
        let resp = self
            .client
            .upload(data.to_vec(), mt, Default::default())
            .await?;
        Ok(crate::result_types::UploadMediaResult {
            url: resp.url,
            direct_path: resp.direct_path,
            media_key: resp.media_key,
            file_sha256: resp.file_sha256,
            file_enc_sha256: resp.file_enc_sha256,
            file_length: resp.file_length as f64,
        })
    }

    /// True streaming encrypt via `MediaEncryptor`: processes plaintext chunk-by-chunk
    /// from JS ReadableStream, encrypts with AES-256-CBC, writes ciphertext to JS WritableStream.
    ///
    /// Peak memory: ~130KB (copy buffer + flush buffer + crypto state).
    #[wasm_bindgen(js_name = encryptMediaStream)]
    pub async fn encrypt_media_stream(
        &self,
        input: web_sys::ReadableStream,
        output: web_sys::WritableStream,
        media_type: crate::result_types::MediaType,
    ) -> Result<crate::result_types::EncryptMediaResult, crate::errors::BridgeError> {
        use futures::SinkExt;
        use futures::StreamExt;
        use wacore::upload::MediaEncryptor;

        let mt: wacore::download::MediaType = media_type.into();

        let rs = wasm_streams::ReadableStream::from_raw(input);
        let mut reader = rs.into_stream();
        let ws = wasm_streams::WritableStream::from_raw(output);
        let mut writer = ws.into_sink();

        const FLUSH_THRESHOLD: usize = 65536;

        let mut enc = MediaEncryptor::new(mt)?;
        let mut out_buf = Vec::with_capacity(FLUSH_THRESHOLD + 16);
        let mut copy_buf = vec![0u8; FLUSH_THRESHOLD];

        while let Some(chunk_result) = reader.next().await {
            let chunk =
                chunk_result.map_err(|e| crate::errors::internal(format!("read error: {e:?}")))?;
            let arr = js_sys::Uint8Array::new(&chunk);
            let len = arr.length() as usize;
            if len == 0 {
                continue;
            }

            if len > copy_buf.len() {
                copy_buf.resize(len, 0);
            }
            arr.copy_to(&mut copy_buf[..len]);

            enc.update(&copy_buf[..len], &mut out_buf);

            if out_buf.len() >= FLUSH_THRESHOLD {
                let js_chunk = js_sys::Uint8Array::from(out_buf.as_slice());
                writer
                    .send(js_chunk.into())
                    .await
                    .map_err(|e| crate::errors::internal(format!("write error: {e:?}")))?;
                out_buf.clear();
            }
        }

        let info = enc.finalize(&mut out_buf)?;

        if !out_buf.is_empty() {
            let js_chunk = js_sys::Uint8Array::from(out_buf.as_slice());
            writer
                .send(js_chunk.into())
                .await
                .map_err(|e| crate::errors::internal(format!("write error: {e:?}")))?;
        }
        writer
            .close()
            .await
            .map_err(|e| crate::errors::internal(format!("close error: {e:?}")))?;

        Ok(crate::result_types::EncryptMediaResult {
            media_key: info.media_key.to_vec(),
            file_sha256: info.file_sha256.to_vec(),
            file_enc_sha256: info.file_enc_sha256.to_vec(),
            file_length: info.file_length as f64,
        })
    }

    /// Upload pre-encrypted media with streaming body.
    ///
    /// `get_body` is a JS function `() => ReadableStream<Uint8Array>` — called
    /// for each upload attempt (retry creates a fresh stream).
    /// Handles CDN failover, auth refresh, and resumable upload (>=5MB).
    #[wasm_bindgen(js_name = uploadEncryptedMediaStream)]
    pub async fn upload_encrypted_media_stream(
        &self,
        get_body: &js_sys::Function,
        media_key: &[u8],
        file_sha256: &[u8],
        file_enc_sha256: &[u8],
        file_length: f64,
        media_type: crate::result_types::MediaType,
    ) -> Result<crate::result_types::UploadMediaResult, crate::errors::BridgeError> {
        let mt: wacore::download::MediaType = media_type.into();
        let file_length = file_length as u64;
        let token = base64_url_encode(file_enc_sha256);
        let mms_type = mt.mms_type();

        let mut force_refresh = false;

        for attempt in 0..=1u32 {
            let media_conn = self.client.refresh_media_conn(force_refresh).await?;

            let mut retry_auth = false;

            for host in &media_conn.hosts {
                // Resumable check for large files (≥5MB)
                if file_length >= 5 * 1024 * 1024 {
                    let check_url = format!(
                        "https://{}/mms/{}/{}?auth={}&token={}&resume=1",
                        host.hostname, mms_type, token, media_conn.auth, token
                    );
                    let check_req = wacore::net::HttpRequest::post(check_url)
                        .with_header("Origin", "https://web.whatsapp.com");
                    if let Ok(resp) = self.client.http_client.execute(check_req).await
                        && resp.status_code < 400
                        && let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&resp.body)
                        && parsed.get("resume").and_then(|v| v.as_str()) == Some("complete")
                        && let (Some(url), Some(dp)) = (
                            parsed.get("url").and_then(|v| v.as_str()),
                            parsed.get("direct_path").and_then(|v| v.as_str()),
                        )
                    {
                        return Ok(crate::result_types::UploadMediaResult {
                            url: url.to_string(),
                            direct_path: dp.to_string(),
                            media_key: media_key.try_into()?,
                            file_sha256: file_sha256.try_into()?,
                            file_enc_sha256: file_enc_sha256.try_into()?,
                            file_length: file_length as f64,
                        });
                    }
                }

                let upload_url = format!(
                    "https://{}/mms/{}/{}?auth={}&token={}",
                    host.hostname, mms_type, token, media_conn.auth, token
                );

                // Get fresh ReadableStream from factory
                let body_stream = get_body
                    .call0(&JsValue::NULL)
                    .map_err(|e| crate::errors::internal(format!("getBody() failed: {e:?}")))?;

                // Try streaming upload via JS HTTP client
                let result = stream_upload_via_js(&self.client, &upload_url, body_stream).await;

                match result {
                    Ok(resp) if resp.status_code < 400 => {
                        let parsed: serde_json::Value = serde_json::from_slice(&resp.body)
                            .map_err(|e| {
                                crate::errors::protocol_violation(format!(
                                    "CDN upload response not JSON: {e}"
                                ))
                            })?;
                        let url = parsed
                            .get("url")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| crate::errors::internal("missing url in response"))?;
                        let dp = parsed
                            .get("direct_path")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                crate::errors::internal("missing direct_path in response")
                            })?;
                        return Ok(crate::result_types::UploadMediaResult {
                            url: url.to_string(),
                            direct_path: dp.to_string(),
                            media_key: media_key.try_into()?,
                            file_sha256: file_sha256.try_into()?,
                            file_enc_sha256: file_enc_sha256.try_into()?,
                            file_length: file_length as f64,
                        });
                    }
                    Ok(resp) if is_auth_error(resp.status_code) && attempt == 0 => {
                        force_refresh = true;
                        retry_auth = true;
                        break;
                    }
                    Ok(resp) => {
                        log::warn!(
                            "Upload to {} failed with status {}",
                            host.hostname,
                            resp.status_code
                        );
                    }
                    Err(e) => {
                        log::warn!("Upload to {} failed: {:?}", host.hostname, e);
                    }
                }
            }

            if !retry_auth {
                break;
            }
        }

        Err(crate::errors::internal("Upload failed on all hosts"))
    }

    // ── State getters ────────────────────────────────────────────────────

    /// Get the current push name.
    #[wasm_bindgen(js_name = getPushName)]
    pub async fn get_push_name(&self) -> String {
        // Sync since whatsapp-rust #808 (cached Arc<Device> snapshot); kept
        // async so the JS surface stays Promise-based.
        self.client.push_name()
    }

    /// Get the own JID (phone number JID) if logged in.
    ///
    /// Returns the non-AD JID (without device suffix), e.g. "559980000014@s.whatsapp.net".
    /// This is the JID used for addressing in messages.
    #[wasm_bindgen(js_name = getJid)]
    pub async fn get_jid(&self) -> Option<String> {
        self.client.pn().map(|j| j.to_non_ad().to_string())
    }

    /// Get the own LID (linked identity) if available.
    ///
    /// Returns the non-AD LID (without device suffix), e.g. "100000012345678@lid".
    #[wasm_bindgen(js_name = getLid)]
    pub async fn get_lid(&self) -> Option<String> {
        self.client.lid().map(|j| j.to_non_ad().to_string())
    }

    /// Get the ADV signed device identity (account), if available.
    /// Exposes the persisted account identity to credential consumers.
    #[wasm_bindgen(js_name = getAccount)]
    pub async fn get_account(&self) -> Result<JsValue, crate::errors::BridgeError> {
        let snapshot = self.client.persistence_manager().get_device_snapshot();
        match &snapshot.account {
            Some(account) => crate::camel_serializer::to_js_value_camel(account)
                .map_err(|e| crate::errors::internal(format!("account serialization: {e:?}"))),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// Returns a snapshot of internal memory diagnostics (cache sizes, session counts, etc.).
    #[wasm_bindgen(js_name = getMemoryDiagnostics)]
    pub async fn get_memory_diagnostics(&self) -> crate::result_types::MemoryDiagnosticsResult {
        let report = self.client.resource_report().await;
        let d = &report.client;
        crate::result_types::MemoryDiagnosticsResult {
            group_cache: d.group_cache.entries as f64,
            group_cache_bytes: d.group_cache.bytes as f64,
            device_registry_cache: d.device_registry_cache.entries as f64,
            device_registry_cache_bytes: d.device_registry_cache.bytes as f64,
            sender_key_device_cache: d.sender_key_device_cache.entries as f64,
            sender_key_device_cache_bytes: d.sender_key_device_cache.bytes as f64,
            group_devices_memo: d.group_devices_memo.entries as f64,
            group_devices_memo_bytes: d.group_devices_memo.bytes as f64,
            lid_pn_lid_entries: d.lid_pn_lid_entries.entries as f64,
            lid_pn_lid_bytes: d.lid_pn_lid_entries.bytes as f64,
            lid_pn_pn_entries: d.lid_pn_pn_entries.entries as f64,
            lid_pn_pn_bytes: d.lid_pn_pn_entries.bytes as f64,
            recent_messages: d.recent_messages.entries as f64,
            recent_messages_bytes: d.recent_messages.bytes as f64,
            message_retry_counts: d.message_retry_counts as f64,
            undecryptable_dispatched: d.undecryptable_dispatched as f64,
            pdo_pending_requests: d.pdo_pending_requests as f64,
            pdo_requested: d.pdo_requested as f64,
            session_locks: d.session_locks as f64,
            chat_lanes: d.chat_lanes as f64,
            group_distribution_locks: d.group_distribution_locks as f64,
            group_distribution_lock_evictions: d.group_distribution_lock_evictions as f64,
            group_distribution_lock_eviction_blocks: d.group_distribution_lock_eviction_blocks
                as f64,
            resend_rate_limiter_chats: d.resend_rate_limiter_chats as f64,
            response_waiters: d.response_waiters as f64,
            node_waiters: d.node_waiters as f64,
            pending_retries: d.pending_retries as f64,
            presence_subscriptions: d.presence_subscriptions as f64,
            app_state_key_requests: d.app_state_key_requests as f64,
            app_state_syncing: d.app_state_syncing as f64,
            signal_cache_sessions: d.signal_sessions.entries as f64,
            signal_cache_sessions_bytes: d.signal_sessions.bytes as f64,
            signal_cache_identities: d.signal_identities.entries as f64,
            signal_cache_identities_bytes: d.signal_identities.bytes as f64,
            signal_cache_sender_keys: d.signal_sender_keys.entries as f64,
            signal_cache_sender_keys_bytes: d.signal_sender_keys.bytes as f64,
            history_sync_tasks: d.history_sync_tasks.entries as f64,
            history_sync_payload_bytes: d.history_sync_tasks.bytes as f64,
            history_sync_peak_tasks: d.history_sync_tasks_peak as f64,
            history_sync_peak_payload_bytes: d.history_sync_payload_bytes_peak as f64,
            chatstate_handlers: d.chatstate_handlers as f64,
            custom_enc_handlers: d.custom_enc_handlers as f64,
            client_estimated_bytes: d.total_estimated_bytes() as f64,
            storage_memory_bytes: report.storage.memory_bytes.map(|v| v as f64),
            storage_pages: report.storage.pages.map(|v| v as f64),
            storage_io_read_bytes: report.storage.io_read_bytes.map(|v| v as f64),
            storage_io_write_bytes: report.storage.io_write_bytes.map(|v| v as f64),
            transport_read_buffer_bytes: report
                .transport
                .and_then(|v| v.read_buffer_bytes)
                .map(|v| v as f64),
            transport_write_buffer_bytes: report
                .transport
                .and_then(|v| v.write_buffer_bytes)
                .map(|v| v as f64),
            transport_tls_state_bytes: report
                .transport
                .and_then(|v| v.tls_state_bytes)
                .map(|v| v as f64),
            http_pool_connections: report
                .http
                .and_then(|v| v.pool_connections)
                .map(|v| v as f64),
            http_pool_buffer_bytes: report
                .http
                .and_then(|v| v.pool_buffer_bytes)
                .map(|v| v as f64),
            http_inflight_bytes: report.http.and_then(|v| v.inflight_bytes).map(|v| v as f64),
            resource_estimated_bytes: report.total_estimated_bytes() as f64,
        }
    }

    /// Allocation churn for work polled by whatsapp-rust's instrumented
    /// runtime. It overlaps the global allocator totals but uniquely separates
    /// core task work from bridge/host-boundary work.
    #[wasm_bindgen(js_name = getCoreAllocationSnapshot)]
    pub fn get_core_allocation_snapshot(
        &self,
    ) -> crate::result_types::CoreAllocationSnapshotResult {
        let Some(meter) = &self.alloc_meter else {
            return crate::result_types::CoreAllocationSnapshotResult {
                enabled: false,
                allocated_bytes: 0.0,
                freed_bytes: 0.0,
                allocations: 0.0,
                net_bytes: 0.0,
            };
        };
        let snapshot = meter.snapshot();
        crate::result_types::CoreAllocationSnapshotResult {
            enabled: true,
            allocated_bytes: snapshot.allocated_bytes as f64,
            freed_bytes: snapshot.freed_bytes as f64,
            allocations: snapshot.allocations as f64,
            net_bytes: snapshot.net_bytes() as f64,
        }
    }

    // ── Signal / low-level protocol ──────────────────────────────────────

    /// Enable or disable raw node forwarding. When enabled, a `raw_node` event
    /// is emitted for every decoded stanza before internal dispatch.
    #[wasm_bindgen(js_name = setRawNodeForwarding)]
    pub fn set_raw_node_forwarding(&self, enabled: bool) {
        let mut lease = self
            .raw_node_lease
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if enabled {
            if lease.is_none() {
                *lease = Some(self.client.acquire_raw_node_forwarding());
            }
        } else {
            lease.take();
        }
    }

    /// Send a raw binary node stanza to WhatsApp servers.
    /// Accepts a JS object matching `{ tag: string, attrs: Record<string, string>, content?: ... }`.
    #[wasm_bindgen(js_name = sendNode)]
    pub async fn send_node(&self, node_js: JsBinaryNode) -> Result<(), crate::errors::BridgeError> {
        let node_js: JsValue = node_js.into();
        let node = js_to_node(&node_js)?;
        self.client
            .send_node(node)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Send an IQ node and return the matching response node.
    #[wasm_bindgen(js_name = queryNode)]
    pub async fn query_node(
        &self,
        node_js: JsBinaryNode,
        timeout_ms: Option<f64>,
    ) -> Result<JsBinaryNode, crate::errors::BridgeError> {
        let node_js: JsValue = node_js.into();
        let node = js_to_node(&node_js)?;
        let timeout = parse_optional_timeout_ms("timeoutMs", timeout_ms)?;
        let response = self.client.send_iq_node(node, timeout).await?;
        Ok(node_ref_to_js(response.get())?.unchecked_into())
    }

    /// Confirm an inbound stanza through the core-owned acknowledgement path.
    #[wasm_bindgen(js_name = acknowledgeStanza)]
    pub async fn acknowledge_stanza(
        &self,
        stanza_js: JsBinaryNode,
    ) -> Result<(), crate::errors::BridgeError> {
        let stanza_js: JsValue = stanza_js.into();
        let stanza = js_to_node(&stanza_js)?;
        self.client
            .acknowledge_stanza(&stanza.as_node_ref())
            .await?;
        Ok(())
    }

    /// Reject an inbound stanza through the core-owned acknowledgement path.
    #[wasm_bindgen(js_name = rejectStanza)]
    pub async fn reject_stanza(
        &self,
        stanza_js: JsBinaryNode,
        error_code: i32,
        failure_reason: Option<i32>,
    ) -> Result<(), crate::errors::BridgeError> {
        let stanza_js: JsValue = stanza_js.into();
        let stanza = js_to_node(&stanza_js)?;
        let reason = whatsapp_rust::NackReason::from(error_code);
        let rejection = if reason == whatsapp_rust::NackReason::InvalidProtobuf {
            whatsapp_rust::StanzaRejection::invalid_protobuf(failure_reason)
        } else {
            whatsapp_rust::StanzaRejection::new(reason)
        };
        self.client
            .reject_stanza(&stanza.as_node_ref(), rejection)
            .await?;
        Ok(())
    }

    /// Request retransmission without acknowledging the original stanza.
    #[wasm_bindgen(js_name = requestMessageRetry)]
    pub async fn request_message_retry(
        &self,
        stanza_js: JsBinaryNode,
        force_include_keys: Option<bool>,
    ) -> Result<(), crate::errors::BridgeError> {
        let stanza_js: JsValue = stanza_js.into();
        let stanza = js_to_node(&stanza_js)?;
        let options = whatsapp_rust::RetryRequestOptions::new()
            .with_force_include_keys(force_include_keys.unwrap_or(false));
        self.client
            .request_message_retry(&stanza.as_node_ref(), options)
            .await?;
        Ok(())
    }

    /// Execute a validated typed USync query through the core-owned operation.
    /// The bridge performs exactly one Serde decode and one Serde encode; it
    /// neither constructs protocol nodes nor translates consumer-facing names.
    #[wasm_bindgen(js_name = queryUsync)]
    pub async fn query_usync(
        &self,
        query: JsUsyncQuery,
    ) -> Result<JsUsyncResponse, crate::errors::BridgeError> {
        let query =
            crate::proto::from_js_value::<whatsapp_rust::usync::UsyncQuery>(query.into(), "query")?;
        let response = self.client.query_usync(query).await?;
        Ok(crate::proto::to_js_value(&response)?.unchecked_into())
    }

    /// Ensure E2E Signal sessions exist for the given JIDs.
    /// Returns true after sessions are established.
    #[wasm_bindgen(js_name = assertSessions)]
    pub async fn assert_sessions(
        &self,
        jids: Vec<String>,
        _force: bool,
    ) -> Result<bool, crate::errors::BridgeError> {
        let parsed: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;
        self.client.signal().assert_sessions(&parsed).await?;
        Ok(true)
    }

    /// Get the list of known devices for the given user JIDs via usync query.
    /// Returns an array of JID strings (one per device).
    #[wasm_bindgen(js_name = getUSyncDevices)]
    pub async fn get_usync_devices(
        &self,
        jids: Vec<String>,
        _use_cache: bool,
        _ignore_zero_devices: bool,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;
        let devices = self.client.signal().get_user_devices(&parsed).await?;
        // Return as JidWithDevice[] = { user: string, device?: number, jid: string }
        let arr = js_sys::Array::new_with_length(devices.len() as u32);
        for (i, jid) in devices.iter().enumerate() {
            let obj = js_sys::Object::new();
            js_sys::Reflect::set(&obj, &"user".into(), &jid.user.as_str().into())
                .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
            if jid.device != 0 {
                js_sys::Reflect::set(&obj, &"device".into(), &(jid.device as f64).into())
                    .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
            }
            js_sys::Reflect::set(&obj, &"jid".into(), &jid.to_string().into())
                .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
            arr.set(i as u32, obj.into());
        }
        Ok(arr.into())
    }

    // ── Signal protocol ──────────────────────────────────────────────────

    /// Encrypt plaintext for a single recipient.
    /// Returns `{ type: "msg"|"pkmsg", ciphertext: Uint8Array }`.
    #[wasm_bindgen(js_name = signalEncryptMessage)]
    pub async fn signal_encrypt_message(
        &self,
        jid: &str,
        data: &[u8],
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed = parse_jid(jid)?;
        let (msg_type, ciphertext) = self.client.signal().encrypt_message(&parsed, data).await?;
        let obj = js_sys::Object::new();
        js_sys::Reflect::set(&obj, &"type".into(), &msg_type.as_wire_str().into())
            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        js_sys::Reflect::set(
            &obj,
            &"ciphertext".into(),
            &js_sys::Uint8Array::from(ciphertext.as_slice()).into(),
        )
        .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        Ok(obj.into())
    }

    /// Decrypt a Signal protocol message. `msg_type` is "msg", "pkmsg", or "skmsg".
    #[wasm_bindgen(js_name = signalDecryptMessage)]
    pub async fn signal_decrypt_message(
        &self,
        jid: &str,
        msg_type: &str,
        ciphertext: &[u8],
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let parsed = parse_jid(jid)?;
        let enc_type = wacore::message_processing::EncType::from_wire(msg_type)
            .ok_or_else(|| crate::errors::internal(format!("invalid msg_type: {msg_type}")))?;
        let plaintext = self
            .client
            .signal()
            .decrypt_message(&parsed, enc_type, ciphertext)
            .await?;
        Ok(js_sys::Uint8Array::from(plaintext.as_slice()))
    }

    /// Encrypt plaintext for a group (sender key).
    /// Returns `{ senderKeyDistributionMessage: Uint8Array, ciphertext: Uint8Array }`.
    #[wasm_bindgen(js_name = signalEncryptGroupMessage)]
    pub async fn signal_encrypt_group_message(
        &self,
        group_jid: &str,
        data: &[u8],
        _me_id: &str,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let parsed = parse_jid(group_jid)?;
        let (skdm, ciphertext) = self
            .client
            .signal()
            .encrypt_group_message(&parsed, data)
            .await?;
        let obj = js_sys::Object::new();
        let skdm_js = match &skdm {
            Some(bytes) => js_sys::Uint8Array::from(bytes.as_slice()).into(),
            None => JsValue::UNDEFINED,
        };
        js_sys::Reflect::set(&obj, &"senderKeyDistributionMessage".into(), &skdm_js)
            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        js_sys::Reflect::set(
            &obj,
            &"ciphertext".into(),
            &js_sys::Uint8Array::from(ciphertext.as_slice()).into(),
        )
        .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        Ok(obj.into())
    }

    /// Decrypt a group (sender-key) message.
    #[wasm_bindgen(js_name = signalDecryptGroupMessage)]
    pub async fn signal_decrypt_group_message(
        &self,
        group_jid: &str,
        author_jid: &str,
        msg: &[u8],
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(author_jid)?;
        let plaintext = self
            .client
            .signal()
            .decrypt_group_message(&group, &sender, msg)
            .await?;
        Ok(js_sys::Uint8Array::from(plaintext.as_slice()))
    }

    /// Install a supplied pairwise pre-key bundle.
    #[wasm_bindgen(js_name = signalInstallPreKeyBundle)]
    pub async fn signal_install_prekey_bundle(
        &self,
        jid: &str,
        input: crate::result_types::SignalSessionBundleInput,
    ) -> Result<(), crate::errors::BridgeError> {
        use wacore::libsignal::protocol::{IdentityKey, PreKeyBundle, PublicKey};

        let parsed = parse_jid(jid)?;
        let identity_key = PublicKey::from_djb_public_key_bytes(&input.identity_key)
            .map(IdentityKey::new)
            .map_err(|error| crate::errors::invalid_arg("identityKey", error.to_string()))?;
        let signed_pre_key = PublicKey::from_djb_public_key_bytes(&input.signed_pre_key.public_key)
            .map_err(|error| {
                crate::errors::invalid_arg("signedPreKey.publicKey", error.to_string())
            })?;
        let pre_key = input
            .pre_key
            .map(|pre_key| {
                PublicKey::from_djb_public_key_bytes(&pre_key.public_key)
                    .map(|public_key| (pre_key.key_id.into(), public_key))
                    .map_err(|error| {
                        crate::errors::invalid_arg("preKey.publicKey", error.to_string())
                    })
            })
            .transpose()?;
        let bundle = PreKeyBundle::new(
            input.registration_id,
            u32::from(parsed.device).into(),
            pre_key,
            input.signed_pre_key.key_id.into(),
            signed_pre_key,
            input.signed_pre_key.signature,
            identity_key,
        )
        .map_err(|error| crate::errors::invalid_arg("bundle", error.to_string()))?;

        self.client
            .signal()
            .install_prekey_bundle(&parsed, &bundle)
            .await?;
        Ok(())
    }

    /// Force-refresh the server's one-time key pool.
    #[wasm_bindgen(js_name = refreshPreKeys)]
    pub async fn refresh_pre_keys(
        &self,
        count: Option<u32>,
    ) -> Result<(), crate::errors::BridgeError> {
        match count {
            Some(count) => {
                self.client
                    .refresh_pre_keys_with_count(count as usize)
                    .await?
            }
            None => self.client.refresh_pre_keys().await?,
        }
        Ok(())
    }

    /// Replenish the server's one-time key pool only when it is low.
    #[wasm_bindgen(js_name = ensurePreKeys)]
    pub async fn ensure_pre_keys(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.ensure_pre_keys().await?;
        Ok(())
    }

    /// Validate the server-side key-bundle digest against local state.
    #[wasm_bindgen(js_name = validateKeyBundle)]
    pub async fn validate_key_bundle(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.validate_digest_key().await?;
        Ok(())
    }

    /// Rotate the signed key advertised by the server.
    #[wasm_bindgen(js_name = rotateSignedKey)]
    pub async fn rotate_signed_key(&self) -> Result<(), crate::errors::BridgeError> {
        self.client.rotate_signed_pre_key().await?;
        Ok(())
    }

    /// Process a raw sender-key distribution payload.
    #[wasm_bindgen(js_name = signalProcessSenderKeyDistribution)]
    pub async fn signal_process_sender_key_distribution(
        &self,
        group_jid: &str,
        sender_jid: &str,
        distribution: &[u8],
    ) -> Result<(), crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        self.client
            .signal()
            .process_sender_key_distribution(&group, &sender, distribution)
            .await?;
        Ok(())
    }

    /// Create the current sender-key distribution payload for a group.
    #[wasm_bindgen(js_name = signalGetSenderKeyDistribution)]
    pub async fn signal_get_sender_key_distribution(
        &self,
        group_jid: &str,
        sender_jid: &str,
    ) -> Result<js_sys::Uint8Array, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        let distribution = self
            .client
            .signal()
            .sender_key_distribution(&group, &sender)
            .await?;
        Ok(crate::wasm_utils::byte_array(&distribution))
    }

    /// Check whether sender-key state exists for a group and sender.
    #[wasm_bindgen(js_name = signalHasSenderKey)]
    pub async fn signal_has_sender_key(
        &self,
        group_jid: &str,
        sender_jid: &str,
    ) -> Result<bool, crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        Ok(self.client.signal().has_sender_key(&group, &sender).await?)
    }

    /// Delete one sender-key chain from live and durable state.
    #[wasm_bindgen(js_name = signalDeleteSenderKey)]
    pub async fn signal_delete_sender_key(
        &self,
        group_jid: &str,
        sender_jid: &str,
    ) -> Result<(), crate::errors::BridgeError> {
        let group = parse_jid(group_jid)?;
        let sender = parse_jid(sender_jid)?;
        self.client
            .signal()
            .delete_sender_key(&group, &sender)
            .await?;
        Ok(())
    }

    /// Inspect the currently open pairwise session for a JID.
    #[wasm_bindgen(js_name = signalGetSessionInfo)]
    pub async fn signal_get_session_info(
        &self,
        jid: &str,
    ) -> Result<Option<crate::result_types::SignalSessionInfoResult>, crate::errors::BridgeError>
    {
        let parsed = parse_jid(jid)?;
        Ok(self
            .client
            .signal()
            .session_info(&parsed)
            .await?
            .map(|info| crate::result_types::SignalSessionInfoResult {
                base_key: info.base_key,
                registration_id: info.registration_id,
            }))
    }

    /// Add linked-identifier mappings through one durable core batch.
    #[wasm_bindgen(js_name = addLidPnMappings)]
    pub async fn add_lid_pn_mappings(
        &self,
        mappings: Vec<crate::result_types::LidPnMappingInput>,
    ) -> Result<u32, crate::errors::BridgeError> {
        let mut pairs = Vec::with_capacity(mappings.len());
        for (index, mapping) in mappings.into_iter().enumerate() {
            let lid = parse_jid(&mapping.lid)?;
            if !lid.server.is_lid_family() {
                return Err(crate::errors::invalid_arg(
                    format!("mappings[{index}].lid"),
                    "must use a linked-identifier namespace",
                ));
            }

            let pn = parse_jid(&mapping.pn)?;
            if !pn.server.is_pn_family() {
                return Err(crate::errors::invalid_arg(
                    format!("mappings[{index}].pn"),
                    "must use a phone-number namespace",
                ));
            }
            pairs.push((lid.user.to_string(), pn.user.to_string()));
        }

        let written = self
            .client
            .add_lid_pn_mappings(pairs, whatsapp_rust::lid_pn_cache::LearningSource::Other)
            .await?;
        u32::try_from(written).map_err(|_| crate::errors::internal("mapping count exceeds u32"))
    }

    /// Move pairwise sessions between phone-number and linked-identifier namespaces.
    #[wasm_bindgen(js_name = signalMigrateSessions)]
    pub async fn signal_migrate_sessions(
        &self,
        from_jid: &str,
        to_jid: &str,
    ) -> Result<crate::result_types::SignalSessionMigrationResult, crate::errors::BridgeError> {
        let from = parse_jid(from_jid)?;
        let to = parse_jid(to_jid)?;
        let result = self.client.signal().migrate_sessions(&from, &to).await?;
        Ok(crate::result_types::SignalSessionMigrationResult {
            migrated: result.migrated as u32,
            skipped: result.skipped as u32,
            total: result.total as u32,
        })
    }

    /// Check whether a Signal session exists for the given JID.
    #[wasm_bindgen(js_name = signalValidateSession)]
    pub async fn signal_validate_session(
        &self,
        jid: &str,
    ) -> Result<bool, crate::errors::BridgeError> {
        let parsed = parse_jid(jid)?;
        self.client
            .signal()
            .validate_session(&parsed)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Delete Signal sessions for the given JIDs.
    #[wasm_bindgen(js_name = signalDeleteSessions)]
    pub async fn signal_delete_sessions(
        &self,
        jids: Vec<String>,
    ) -> Result<(), crate::errors::BridgeError> {
        let parsed: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;
        self.client
            .signal()
            .delete_sessions(&parsed)
            .await
            .map_err(crate::errors::BridgeError::from)
    }

    /// Look up the LID JID corresponding to a given phone number JID.
    ///
    /// Accepts a bare phone number (treated as PN), a `<phone>@s.whatsapp.net`
    /// JID, or any LID/PN JID. Returns the full LID JID string (e.g.
    /// `100000012345678@lid`) or `null` when no mapping is known. Backed by
    /// the core's cache-aside `get_lid_pn_entry`: hits the in-memory cache
    /// first, then falls through to `backend.get_pn_mapping(user)` so a JS
    /// `JsStoreCallbacks` backend without a list primitive still resolves
    /// every persisted mapping without an extra usync round trip.
    #[wasm_bindgen(js_name = lidForPn)]
    pub async fn lid_for_pn(
        &self,
        jid: &str,
    ) -> Result<Option<String>, crate::errors::BridgeError> {
        let parsed = if jid.contains('@') {
            parse_jid(jid)?
        } else {
            Jid::pn(jid)
        };
        Ok(self
            .client
            .get_lid_pn_entry(&parsed)
            .await?
            .map(|e| format!("{}@lid", e.lid)))
    }

    /// Look up the phone number JID corresponding to a given LID JID.
    ///
    /// Accepts a bare LID user-part, a `<user>@lid` JID, or any LID/PN JID.
    /// Returns the full PN JID string (e.g. `559980000001@s.whatsapp.net`) or
    /// `null` when no mapping is known. Same cache-aside semantics as
    /// `lidForPn` — see that doc.
    #[wasm_bindgen(js_name = pnForLid)]
    pub async fn pn_for_lid(
        &self,
        jid: &str,
    ) -> Result<Option<String>, crate::errors::BridgeError> {
        let parsed = if jid.contains('@') {
            parse_jid(jid)?
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
    #[wasm_bindgen(js_name = jidToSignalProtocolAddress)]
    pub fn jid_to_signal_protocol_address(
        &self,
        jid: &str,
    ) -> Result<String, crate::errors::BridgeError> {
        use wacore::types::jid::JidExt;
        let parsed = parse_jid(jid)?;
        Ok(parsed.to_protocol_address_string())
    }

    // ── Participant node creation ────────────────────────────────────────

    /// Create encrypted participant `<to>` nodes for recipient JIDs.
    /// Returns `{ nodes: [...], shouldIncludeDeviceIdentity: boolean }`.
    /// Use `encodeProto('Message', obj)` on the JS side to produce the bytes.
    #[wasm_bindgen(js_name = createParticipantNodesBytes)]
    pub async fn create_participant_nodes_bytes(
        &self,
        jids: Vec<String>,
        bytes: &[u8],
        _extra_attrs: JsValue,
    ) -> Result<JsValue, crate::errors::BridgeError> {
        let recipient_jids: Vec<wacore_binary::jid::Jid> = jids
            .iter()
            .map(|j| parse_jid(j))
            .collect::<Result<_, _>>()?;

        let msg = waproto::codec::message_decode(bytes)
            .map_err(|e| crate::errors::internal(format!("invalid message bytes: {e}")))?;

        let (nodes, should_include_device_identity) = self
            .client
            .signal()
            .create_participant_nodes(&recipient_jids, &msg)
            .await?;

        let obj = js_sys::Object::new();
        let nodes_js = nodes_to_js_array(&nodes)
            .map_err(|e| crate::errors::internal(format!("node serialization failed: {e:?}")))?;
        js_sys::Reflect::set(&obj, &"nodes".into(), &nodes_js)
            .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        js_sys::Reflect::set(
            &obj,
            &"shouldIncludeDeviceIdentity".into(),
            &should_include_device_identity.into(),
        )
        .map_err(|e| crate::errors::internal(format!("{e:?}")))?;
        Ok(obj.into())
    }

    // ── Raw transport ────────────────────────────────────────────────────

    /// Send pre-marshaled bytes through the noise socket.
    #[wasm_bindgen(js_name = sendRawMessage)]
    pub async fn send_raw_message(&self, data: &[u8]) -> Result<(), crate::errors::BridgeError> {
        self.client
            .send_raw_bytes(data.to_vec())
            .await
            .map_err(crate::errors::BridgeError::from)
    }
}

impl Drop for WasmWhatsAppClient {
    /// Teardown for the `free()` path (explicit or via wasm-bindgen's
    /// `FinalizationRegistry`). When the caller skipped `disconnect()`, this
    /// guarantees the detached background tasks observe shutdown and the
    /// transport gets closed — without it the orphaned tasks keep awaiting
    /// JsFutures whose `Closure` state has been freed, which surfaces later
    /// as `RuntimeError: Out of bounds memory access` on the shared WASM
    /// heap. Callers should still prefer `await disconnect()` first; this
    /// is the safety net for the GC path.
    fn drop(&mut self) {
        // Signal shutdown to `Arc<Client>` synchronously. Detached children
        // (every `.detach()` in `whatsapp_rust/src/client.rs` — keepalive
        // loop, message processors, retry loops, …) observe `is_running` /
        // `shutdown_notifier` and exit on their next poll.
        self.client.signal_shutdown_sync();

        // Abort the bridge-owned wrappers (run loop + sync worker + saver).
        // The async cleanup task spawned below holds `Arc<Client>` so the
        // aborted futures have valid state to unwind through.
        for slot in [
            &self.saver_handle,
            &self.run_handle,
            &self.connection_handle,
            &self.sync_worker_handle,
        ] {
            if let Some(handle) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                handle.abort();
            }
        }

        // Drive teardown event-driven: `disconnect()` cancels the transport
        // (closing the channels detached children are parked on) and runs
        // `outbound_flush` to drain pending writes. `done` is awaited by the
        // next `create_whatsapp_client` so a new client can't start sharing
        // the heap until this completes.
        let client = self.client.clone();
        let done = register_drop_cleanup();
        wasm_bindgen_futures::spawn_local(async move {
            client.disconnect().await;
            let _ = done.send(());
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn participants_update(
    client: &whatsapp_rust::Client,
    jid: &str,
    participants: Vec<String>,
    action: crate::result_types::GroupParticipantAction,
    include_linked_groups_on_remove: bool,
) -> Result<Vec<crate::result_types::ParticipantChangeResult>, crate::errors::BridgeError> {
    use crate::result_types::GroupParticipantAction;

    let group_jid = parse_jid(jid)?;
    let participant_jids = participants
        .iter()
        .map(|participant| parse_jid(participant))
        .collect::<Result<Vec<_>, _>>()?;

    let responses = match action {
        GroupParticipantAction::Add => {
            client
                .groups()
                .add_participants(&group_jid, &participant_jids)
                .await?
        }
        GroupParticipantAction::Remove if include_linked_groups_on_remove => {
            client
                .community()
                .remove_participants(&group_jid, &participant_jids)
                .await?
        }
        GroupParticipantAction::Remove => {
            client
                .groups()
                .remove_participants(&group_jid, &participant_jids)
                .await?
        }
        GroupParticipantAction::Promote => {
            client
                .groups()
                .promote_participants(&group_jid, &participant_jids)
                .await?
        }
        GroupParticipantAction::Demote => {
            client
                .groups()
                .demote_participants(&group_jid, &participant_jids)
                .await?
        }
        GroupParticipantAction::Modify => {
            return Err(crate::errors::BridgeError::InvalidArgument {
                field: "action".into(),
                reason:
                    "modify represents a received participant identity change and cannot be sent"
                        .into(),
            });
        }
    };

    Ok(responses.iter().map(participant_change_to_result).collect())
}

fn participant_change_to_result(
    response: &whatsapp_rust::features::ParticipantChangeResponse,
) -> crate::result_types::ParticipantChangeResult {
    crate::result_types::ParticipantChangeResult {
        jid: response.jid.to_string(),
        status: response.status.clone(),
        error: response.error.clone(),
        phone_number: response.phone_number.as_ref().map(ToString::to_string),
        username: response.username.clone(),
        add_request: response.add_request.as_ref().map(|request| {
            crate::result_types::ParticipantAddRequestResult {
                code: request.code.clone(),
                expiration: request.expiration as f64,
            }
        }),
    }
}

fn community_link_result(
    succeeded: Vec<Jid>,
    failed: Vec<(Jid, u32)>,
) -> crate::result_types::CommunityLinkResult {
    crate::result_types::CommunityLinkResult {
        succeeded: succeeded.into_iter().map(|jid| jid.to_string()).collect(),
        failed: failed
            .into_iter()
            .map(
                |(jid, error)| crate::result_types::CommunityLinkFailureResult {
                    jid: jid.to_string(),
                    error: error as f64,
                },
            )
            .collect(),
    }
}

/// Convert GroupMetadata to a typed result struct.
fn group_metadata_to_result(
    metadata: &whatsapp_rust::features::GroupMetadata,
) -> crate::result_types::GroupMetadataResult {
    use crate::result_types::{
        GroupEphemeralSettingsResult, GroupGrowthLockInfoResult, GroupMetadataParticipant,
        GroupMetadataResult,
    };
    GroupMetadataResult {
        id: metadata.id.to_string(),
        subject: metadata.subject.to_string(),
        notify: metadata.notify.clone(),
        participants: metadata
            .participants
            .iter()
            .map(|p| GroupMetadataParticipant {
                jid: p.jid.to_string(),
                phone_number: p.phone_number.as_ref().map(|pn| pn.to_string()),
                lid: p.lid.as_ref().map(|lid| lid.to_string()),
                username: p.username.as_ref().map(ToString::to_string),
                participant_type: p.participant_type.as_str().to_string(),
                is_admin: p.is_admin(),
                is_super_admin: p.is_super_admin(),
            })
            .collect(),
        addressing_mode: metadata.addressing_mode.as_str().to_string(),
        creator: metadata.creator.as_ref().map(|j| j.to_string()),
        creator_pn: metadata.creator_pn.as_ref().map(|j| j.to_string()),
        creator_username: metadata.creator_username.clone(),
        creator_country_code: metadata.creator_country_code.clone(),
        creation_time: metadata.creation_time.map(|v| v as f64),
        subject_time: metadata.subject_time.map(|v| v as f64),
        subject_owner: metadata.subject_owner.as_ref().map(|j| j.to_string()),
        subject_owner_pn: metadata.subject_owner_pn.as_ref().map(|j| j.to_string()),
        subject_owner_username: metadata.subject_owner_username.clone(),
        description: metadata.description.clone(),
        description_id: metadata.description_id.clone(),
        description_owner: metadata.description_owner.as_ref().map(|j| j.to_string()),
        description_owner_pn: metadata
            .description_owner_pn
            .as_ref()
            .map(|j| j.to_string()),
        description_owner_username: metadata.description_owner_username.clone(),
        description_time: metadata.description_time.map(|v| v as f64),
        is_locked: metadata.is_locked,
        is_announcement: metadata.is_announcement,
        ephemeral: metadata
            .ephemeral
            .map(|settings| GroupEphemeralSettingsResult {
                expiration: settings.expiration.map(|v| v as f64),
                trigger: settings.trigger.map(|v| v as f64),
            }),
        membership_approval: metadata.membership_approval,
        member_add_mode: metadata
            .member_add_mode
            .as_ref()
            .map(|m| m.as_str().to_string()),
        member_link_mode: metadata
            .member_link_mode
            .as_ref()
            .map(|m| m.as_str().to_string()),
        size: metadata.size.map(|v| v as f64),
        is_parent_group: metadata.is_parent_group,
        parent_group_jid: metadata.parent_group_jid.as_ref().map(|j| j.to_string()),
        is_default_sub_group: metadata.is_default_sub_group,
        is_general_chat: metadata.is_general_chat,
        allow_non_admin_sub_group_creation: metadata.allow_non_admin_sub_group_creation,
        no_frequently_forwarded: metadata.no_frequently_forwarded,
        member_share_history_mode: metadata
            .member_share_history_mode
            .as_ref()
            .map(|m| m.as_str().to_string()),
        growth_locked: metadata
            .growth_locked
            .as_ref()
            .map(|lock| GroupGrowthLockInfoResult {
                lock_type: lock.lock_type.clone(),
                expiration: lock.expiration as f64,
            }),
        is_suspended: metadata.is_suspended,
        allow_admin_reports: metadata.allow_admin_reports,
        is_hidden_group: metadata.is_hidden_group,
        is_incognito: metadata.is_incognito,
        has_group_history: metadata.has_group_history,
        is_limit_sharing_enabled: metadata.is_limit_sharing_enabled,
    }
}

// ---------------------------------------------------------------------------
// Poll vote decryption — standalone functions (not on WasmWhatsAppClient)
// ---------------------------------------------------------------------------

/// Decrypt a poll vote. Returns selected option names as a string array.
#[wasm_bindgen(js_name = decryptPollVote)]
pub fn decrypt_poll_vote(
    enc_payload: &[u8],
    enc_iv: &[u8],
    message_secret: &[u8],
    poll_msg_id: &str,
    poll_creator_jid: &str,
    voter_jid: &str,
    option_names: Vec<String>,
) -> Result<Vec<String>, crate::errors::BridgeError> {
    let creator = parse_jid(poll_creator_jid)?;
    let voter = parse_jid(voter_jid)?;

    // Client-less context: no LID/PN swap fallback available (that needs a
    // `Client`), so call the underlying primitive directly with `None`.
    let creator_str = creator.to_non_ad().to_string();
    let voter_str = voter.to_non_ad().to_string();
    let selected_hashes = wacore::poll::decrypt_poll_vote_with_fallback(
        wacore::poll::PollVoteCiphertext {
            enc_payload,
            enc_iv,
        },
        message_secret,
        poll_msg_id,
        wacore::poll::PollVoteAddressing {
            poll_creator_jid: &creator_str,
            voter_jid: &voter_str,
        },
        None,
    )?;

    // Map hashes back to option names
    let option_map: Vec<([u8; 32], &str)> = option_names
        .iter()
        .map(|n| (wacore::poll::compute_option_hash(n), n.as_str()))
        .collect();

    let mut result = Vec::new();
    for hash in &selected_hashes {
        if let Ok(hash_arr) = <[u8; 32]>::try_from(hash.as_slice())
            && let Some((_, name)) = option_map.iter().find(|(h, _)| *h == hash_arr)
        {
            result.push(name.to_string());
        }
    }
    Ok(result)
}

/// Parse a JID string, returning a JS error on failure.
fn parse_jid(jid: &str) -> Result<Jid, crate::errors::BridgeError> {
    jid.parse().map_err(crate::errors::BridgeError::from)
}

/// Carry the bot directory across as the core grouped it: every section, with
/// its presentation metadata. `BotList::flatten` is the core's own collapse and
/// stays a consumer's call.
fn bot_list_to_result(
    list: &whatsapp_rust::features::BotList,
) -> crate::result_types::BotListResult {
    use crate::result_types::{
        BotDefaultResult, BotListEntryResult, BotListSectionResult, BotThemeResult,
    };

    crate::result_types::BotListResult {
        version: list.version.as_str().to_owned(),
        bhash: list.bhash.clone(),
        default_bot: list.default_bot.as_ref().map(|bot| BotDefaultResult {
            jid: bot.jid.to_string(),
            persona_id: bot.persona_id.clone(),
        }),
        sections: list
            .sections
            .iter()
            .map(|section| BotListSectionResult {
                name: section.name.clone(),
                section_type: section.section_type.as_str().to_owned(),
                display_type: section
                    .display_type
                    .as_ref()
                    .map(|kind| kind.as_str().to_owned()),
                bots: section
                    .bots
                    .iter()
                    .map(|bot| BotListEntryResult {
                        jid: bot.jid.to_string(),
                        persona_id: bot.persona_id.clone(),
                        card_title: bot.card_title.clone(),
                        count: bot.count.map(|value| value as f64),
                        themes: bot
                            .themes
                            .iter()
                            .map(|theme| BotThemeResult {
                                mode: theme.mode.as_str().to_owned(),
                                background: theme.background.clone(),
                                primary_text: theme.primary_text.clone(),
                                secondary_text: theme.secondary_text.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Node ↔ BinaryNode conversion
// ---------------------------------------------------------------------------
// The host-facing binary-node object uses `{ tag, attrs, content }`, while
// wacore's `Node` stores compact attributes and tagged content. These
// converters preserve the neutral wire representation across the boundary.

/// Convert a wacore `Node` → JS `BinaryNode` object `{ tag, attrs, content }`.
fn node_to_js(node: &wacore_binary::node::Node) -> Result<JsValue, JsValue> {
    let obj = js_sys::Object::new();
    js_sys::Reflect::set(&obj, &"tag".into(), &node.tag.as_ref().into())?;

    // attrs: Record<string, string> — flatten Vec<(key, NodeValue)> to object
    let attrs_obj = js_sys::Object::new();
    for (key, value) in node.attrs.iter() {
        let val_str: String = value.as_str().into_owned();
        js_sys::Reflect::set(&attrs_obj, &key.as_ref().into(), &val_str.into())?;
    }
    js_sys::Reflect::set(&obj, &"attrs".into(), &attrs_obj.into())?;

    // content: BinaryNode[] | string | Uint8Array | undefined
    if let Some(content) = &node.content {
        let content_js = match content {
            wacore_binary::node::NodeContent::Nodes(nodes) => {
                let arr = js_sys::Array::new_with_length(nodes.len() as u32);
                for (i, child) in nodes.iter().enumerate() {
                    arr.set(i as u32, node_to_js(child)?);
                }
                arr.into()
            }
            wacore_binary::node::NodeContent::Bytes(bytes) => {
                js_sys::Uint8Array::from(bytes.as_slice()).into()
            }
            wacore_binary::node::NodeContent::String(s) => JsValue::from_str(s),
        };
        js_sys::Reflect::set(&obj, &"content".into(), &content_js)?;
    }

    Ok(obj.into())
}

/// Convert a yoke-borrowed `NodeRef` → JS `BinaryNode` object `{ tag, attrs, content }`.
/// Zero-copy: reads directly from the decoded buffer without cloning to `Node`.
fn node_ref_to_js(node: &wacore_binary::node::NodeRef<'_>) -> Result<JsValue, JsValue> {
    let obj = js_sys::Object::new();
    js_sys::Reflect::set(&obj, &"tag".into(), &JsValue::from_str(&node.tag))?;

    let attrs_obj = js_sys::Object::new();
    for (key, value) in node.attrs_iter() {
        let js_val = match value {
            wacore_binary::node::ValueRef::String(s) => JsValue::from_str(s),
            wacore_binary::node::ValueRef::Jid(j) => {
                let mut s = String::with_capacity(j.user.len() + 20);
                wacore_binary::push_jid_to_string(&j.user, j.server, j.agent, j.device, &mut s);
                JsValue::from_str(&s)
            }
        };
        js_sys::Reflect::set(&attrs_obj, &JsValue::from_str(key), &js_val)?;
    }
    js_sys::Reflect::set(&obj, &"attrs".into(), &attrs_obj.into())?;

    if let Some(content) = node.content.as_ref() {
        let content_js = match content {
            wacore_binary::node::NodeContentRef::Nodes(children) => {
                let arr = js_sys::Array::new_with_length(children.len() as u32);
                for (i, child) in children.iter().enumerate() {
                    arr.set(i as u32, node_ref_to_js(child)?);
                }
                arr.into()
            }
            wacore_binary::node::NodeContentRef::Bytes(bytes) => {
                js_sys::Uint8Array::from(bytes.as_ref()).into()
            }
            wacore_binary::node::NodeContentRef::String(s) => JsValue::from_str(s.as_ref()),
        };
        js_sys::Reflect::set(&obj, &"content".into(), &content_js)?;
    }

    Ok(obj.into())
}

/// Convert a JS `BinaryNode` object → wacore `Node`.
fn js_to_node(val: &JsValue) -> Result<wacore_binary::node::Node, crate::errors::BridgeError> {
    use std::borrow::Cow;
    use wacore_binary::node::{Attrs, Node, NodeContent, NodeValue};

    let tag: String = js_sys::Reflect::get(val, &"tag".into())
        .map_err(|e| crate::errors::internal(format!("missing tag: {e:?}")))?
        .as_string()
        .ok_or_else(|| crate::errors::internal("tag must be a string"))?;

    // Parse attrs: Record<string, string> → Vec<(Cow, NodeValue)>
    let attrs_val = js_sys::Reflect::get(val, &"attrs".into()).unwrap_or(JsValue::UNDEFINED);
    let mut attrs = Attrs::new();
    if attrs_val.is_object() && !attrs_val.is_undefined() && !attrs_val.is_null() {
        let keys = js_sys::Object::keys(&js_sys::Object::from(attrs_val.clone()));
        for i in 0..keys.length() {
            let key = keys.get(i).as_string().unwrap_or_default();
            let value = js_sys::Reflect::get(&attrs_val, &key.as_str().into())
                .unwrap_or(JsValue::UNDEFINED);
            let val_str = value.as_string().unwrap_or_default();
            // Try to parse as JID if it contains '@'
            if val_str.contains('@')
                && let Ok(jid) = val_str.parse::<wacore_binary::jid::Jid>()
            {
                attrs.push(Cow::Owned(key), NodeValue::Jid(jid));
                continue;
            }
            attrs.push(Cow::Owned(key), NodeValue::String(val_str.into()));
        }
    }

    // Parse content
    let content_val = js_sys::Reflect::get(val, &"content".into()).unwrap_or(JsValue::UNDEFINED);
    let content = if content_val.is_undefined() || content_val.is_null() {
        None
    } else if content_val.is_string() {
        Some(NodeContent::String(
            content_val.as_string().unwrap_or_default().into(),
        ))
    } else if content_val.is_instance_of::<js_sys::Uint8Array>() {
        let arr = js_sys::Uint8Array::from(content_val);
        Some(NodeContent::Bytes(arr.to_vec()))
    } else if js_sys::Array::is_array(&content_val) {
        let arr = js_sys::Array::from(&content_val);
        let mut children = Vec::with_capacity(arr.length() as usize);
        for i in 0..arr.length() {
            children.push(js_to_node(&arr.get(i))?);
        }
        Some(NodeContent::Nodes(children))
    } else {
        None
    };

    Ok(Node::new(Cow::Owned(tag), attrs, content))
}

#[cfg(test)]
mod node_roundtrip_tests {
    use super::*;

    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// A `{ tag, attrs, content }` node with `count` attributes, the shape a
    /// host hands `sendNode`. Values carry no `@` so none is read as a JID.
    fn js_node(count: usize) -> JsValue {
        const KEYS: [&str; 5] = ["to", "id", "type", "t", "edit"];
        let attrs = js_sys::Object::new();
        for (i, key) in KEYS.iter().take(count).enumerate() {
            js_sys::Reflect::set(&attrs, &(*key).into(), &format!("v{i}").into())
                .expect("the attribute object accepts a key");
        }
        let node = js_sys::Object::new();
        js_sys::Reflect::set(&node, &"tag".into(), &"message".into()).expect("tag is settable");
        js_sys::Reflect::set(&node, &"attrs".into(), &attrs.into()).expect("attrs is settable");
        js_sys::Reflect::set(&node, &"content".into(), &"body".into())
            .expect("content is settable");
        node.into()
    }

    fn attrs_of(node: &JsValue) -> Vec<(String, String)> {
        let attrs = js_sys::Reflect::get(node, &"attrs".into()).expect("the node carries attrs");
        let keys = js_sys::Object::keys(&js_sys::Object::from(attrs.clone()));
        (0..keys.length())
            .map(|i| {
                let key = keys.get(i).as_string().expect("attribute keys are strings");
                let value = js_sys::Reflect::get(&attrs, &key.as_str().into())
                    .expect("the key was just listed")
                    .as_string()
                    .expect("attribute values cross as strings");
                (key, value)
            })
            .collect()
    }

    /// Attribute storage is inline up to three and spills at four
    /// (whatsapp-rust #1253). Both sides of that boundary have to survive the
    /// bridge's own conversions, so exercise the encode and the zero-copy
    /// decode path at three and at four.
    #[test]
    fn attributes_survive_the_inline_and_the_spilled_layout() {
        for count in [3, 4] {
            let sent = js_node(count);
            let node = js_to_node(&sent).expect("the JS node converts");
            let packed = wacore_binary::marshal::marshal(&node).expect("the node marshals");
            let decoded = wacore_binary::marshal::unmarshal_packed_ref(&packed)
                .expect("marshal output unmarshals");
            let back = node_ref_to_js(&decoded).expect("the decoded node crosses back");

            assert_eq!(attrs_of(&back), attrs_of(&sent), "{count} attributes");
            assert_eq!(
                js_sys::Reflect::get(&back, &"tag".into())
                    .expect("the node carries a tag")
                    .as_string(),
                Some("message".to_owned()),
            );
        }
    }
}

/// Convert an array of wacore Nodes to JS BinaryNode array.
fn nodes_to_js_array(nodes: &[wacore_binary::node::Node]) -> Result<JsValue, JsValue> {
    let arr = js_sys::Array::new_with_length(nodes.len() as u32);
    for (i, node) in nodes.iter().enumerate() {
        arr.set(i as u32, node_to_js(node)?);
    }
    Ok(arr.into())
}

/// Base64-URL-safe (no padding) encoding for upload tokens.
fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// Check if an HTTP status code is a media auth error (401/403).
fn is_auth_error(status: u16) -> bool {
    matches!(status, 401 | 403)
}

/// Consume a JS ReadableStream and upload via the HTTP client.
///
/// Buffers the stream into memory because `HttpClient::execute` takes `Vec<u8>`.
async fn stream_upload_via_js(
    client: &whatsapp_rust::Client,
    url: &str,
    body_stream: JsValue,
) -> Result<wacore::net::HttpResponse, crate::errors::BridgeError> {
    use futures::StreamExt;

    let rs = wasm_streams::ReadableStream::from_raw(body_stream.unchecked_into());
    let mut stream = rs.into_stream();

    let mut body_bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let arr = js_sys::Uint8Array::new(&chunk);
        let start = body_bytes.len();
        body_bytes.resize(start + arr.length() as usize, 0);
        arr.copy_to(&mut body_bytes[start..]);
    }

    let request = wacore::net::HttpRequest::post(url.to_string())
        .with_header("Content-Type", "application/octet-stream")
        .with_header("Origin", "https://web.whatsapp.com")
        .with_body(body_bytes);

    client
        .http_client
        .execute(request)
        .await
        .map_err(Into::into)
}

fn parse_jid_and_msg_bytes(
    jid: &str,
    bytes: &[u8],
) -> Result<(Jid, waproto::whatsapp::Message), crate::errors::BridgeError> {
    let to = parse_jid(jid)?;
    let msg = waproto::codec::message_decode(bytes)
        .map_err(|e| crate::errors::internal(format!("invalid message bytes: {e}")))?;
    Ok((to, msg))
}

#[inline]
fn freshness(refresh: bool) -> whatsapp_rust::Freshness {
    if refresh {
        whatsapp_rust::Freshness::Refresh
    } else {
        whatsapp_rust::Freshness::CachePreferred
    }
}

fn js_node_array_to_vec(
    nodes: JsBinaryNodeArray,
) -> Result<Vec<wacore_binary::node::Node>, crate::errors::BridgeError> {
    let nodes: JsValue = nodes.into();
    if !js_sys::Array::is_array(&nodes) {
        return Err(crate::errors::internal("extra nodes must be an array"));
    }

    let nodes = js_sys::Array::from(&nodes);
    let mut result = Vec::with_capacity(nodes.length() as usize);
    for node in nodes.iter() {
        result.push(js_to_node(&node)?);
    }
    Ok(result)
}

async fn send_message_with_options(
    client: &whatsapp_rust::Client,
    to: Jid,
    msg: waproto::whatsapp::Message,
    options: whatsapp_rust::SendOptions,
) -> Result<String, crate::errors::BridgeError> {
    let result = client.send_message_with_options(to, msg, options).await?;
    Ok(result.message_id)
}

async fn send_status_message_with_options(
    client: &whatsapp_rust::Client,
    bytes: &[u8],
    recipients: Vec<String>,
    options: whatsapp_rust::StatusSendOptions,
) -> Result<String, crate::errors::BridgeError> {
    let msg = waproto::codec::message_decode(bytes)
        .map_err(|e| crate::errors::internal(format!("invalid message bytes: {e}")))?;
    let recipients = recipients
        .iter()
        .map(|jid| parse_jid(jid))
        .collect::<Result<Vec<_>, _>>()?;
    let result = client.status().send_raw(msg, &recipients, options).await?;
    Ok(result.message_id)
}

/// Convert NewsletterMetadata to a typed result struct.
fn newsletter_metadata_to_result(
    meta: &whatsapp_rust::features::NewsletterMetadata,
) -> crate::result_types::NewsletterMetadataResult {
    crate::result_types::NewsletterMetadataResult {
        jid: meta.jid.to_string(),
        name: meta.name.to_string(),
        description: meta.description.clone(),
        subscriber_count: meta.subscriber_count as f64,
        verification: format!("{:?}", meta.verification),
        state: format!("{:?}", meta.state),
        picture_url: meta.picture_url.clone(),
        preview_url: meta.preview_url.clone(),
        invite_code: meta.invite_code.clone(),
        role: meta.role.as_ref().map(|r| format!("{:?}", r)),
        creation_time: meta.creation_time.map(|v| v as f64),
    }
}

fn business_profile_to_result(
    p: &wacore::iq::business::BusinessProfile,
) -> crate::result_types::BusinessProfileResult {
    crate::result_types::BusinessProfileResult {
        wid: p.wid.as_ref().map(|j| j.to_string()),
        description: p.description.clone(),
        email: p.email.clone(),
        website: p.website.clone(),
        categories: p
            .categories
            .iter()
            .map(|c| crate::result_types::BusinessCategoryResult {
                id: c.id.clone(),
                name: c.name.clone(),
            })
            .collect(),
        address: p.address.clone(),
        business_hours: crate::result_types::BusinessHoursResult {
            timezone: p.business_hours.timezone.clone(),
            business_config: p.business_hours.business_config.as_ref().map(|configs| {
                configs
                    .iter()
                    .map(|c| crate::result_types::BusinessHoursConfigResult {
                        day_of_week: c.day_of_week.to_string(),
                        mode: c.mode.to_string(),
                        open_time: c.open_time.map(f64::from),
                        close_time: c.close_time.map(f64::from),
                    })
                    .collect()
            }),
        },
    }
}

// ---------------------------------------------------------------------------
// Cache config builder
// ---------------------------------------------------------------------------

/// Build a Rust `CacheConfig` from an optional JS `CacheConfig` object.
/// Omitted fields keep their defaults.
fn build_cache_config(js: Option<&JsValue>) -> Result<whatsapp_rust::CacheConfig, JsValue> {
    use crate::js_cache_store::JsCacheStoreAdapter;

    let mut config = whatsapp_rust::CacheConfig::default();

    let js = match js {
        Some(v) if !v.is_null() && !v.is_undefined() => v,
        _ => return Ok(config),
    };

    // Global store (applied to all pluggable caches unless overridden per-cache)
    let global_store = js_sys::Reflect::get(js, &"store".into())
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
        .and_then(|v| {
            JsCacheStoreAdapter::from_js(&v)
                .ok()
                .map(|a| Arc::new(a) as Arc<dyn whatsapp_rust::CacheStore>)
        });

    if let Some(ref store) = global_store {
        config.cache_stores = whatsapp_rust::CacheStores::all(store.clone());
    }

    // Per-cache overrides
    apply_cache_entry(
        js,
        "group",
        &mut config.group_cache,
        &mut config.cache_stores.group_cache,
    )?;
    apply_cache_entry(
        js,
        "deviceRegistry",
        &mut config.device_registry_cache,
        &mut config.cache_stores.device_registry_cache,
    )?;
    apply_cache_entry(
        js,
        "lidPn",
        &mut config.lid_pn_cache,
        &mut config.cache_stores.lid_pn_cache,
    )?;
    apply_cache_entry_simple(js, "recentMessages", &mut config.recent_messages)?;
    apply_cache_entry_simple(js, "messageRetry", &mut config.message_retry_counts)?;

    Ok(config)
}

/// Apply JS overrides to a cache entry that supports custom stores.
fn apply_cache_entry(
    parent: &JsValue,
    key: &str,
    entry: &mut whatsapp_rust::CacheEntryConfig,
    store_slot: &mut Option<std::sync::Arc<dyn whatsapp_rust::CacheStore>>,
) -> Result<(), JsValue> {
    use crate::js_cache_store::JsCacheStoreAdapter;

    let obj = match js_sys::Reflect::get(parent, &key.into()) {
        Ok(v) if !v.is_undefined() && !v.is_null() => v,
        _ => return Ok(()),
    };

    apply_ttl_capacity(&obj, entry)?;

    // Per-cache store override (takes priority over global)
    if let Ok(store_val) = js_sys::Reflect::get(&obj, &"store".into())
        && !store_val.is_undefined()
        && !store_val.is_null()
        && let Ok(adapter) = JsCacheStoreAdapter::from_js(&store_val)
    {
        *store_slot = Some(Arc::new(adapter) as Arc<dyn whatsapp_rust::CacheStore>);
    }

    Ok(())
}

/// Apply JS overrides to a simple cache entry (no custom store support).
fn apply_cache_entry_simple(
    parent: &JsValue,
    key: &str,
    entry: &mut whatsapp_rust::CacheEntryConfig,
) -> Result<(), JsValue> {
    let obj = match js_sys::Reflect::get(parent, &key.into()) {
        Ok(v) if !v.is_undefined() && !v.is_null() => v,
        _ => return Ok(()),
    };

    apply_ttl_capacity(&obj, entry)
}

/// Shared: apply ttlSecs and capacity from a JS object to a CacheEntryConfig.
fn apply_ttl_capacity(
    obj: &JsValue,
    entry: &mut whatsapp_rust::CacheEntryConfig,
) -> Result<(), JsValue> {
    use std::time::Duration;

    if let Ok(ttl) = js_sys::Reflect::get(obj, &"ttlSecs".into())
        && let Some(secs) = ttl.as_f64()
    {
        entry.timeout = if secs > 0.0 {
            Some(Duration::from_secs(secs as u64))
        } else {
            None
        };
    }

    if let Ok(cap) = js_sys::Reflect::get(obj, &"capacity".into())
        && let Some(c) = cap.as_f64()
    {
        entry.capacity = c as u64;
    }

    Ok(())
}

#[cfg(test)]
mod packed_batch_channel_tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// A receipt batch as the dispatch loop crosses it, with `id` inline so the
    /// bytes identify which batch a callback saw.
    fn cross_receipt_batch(id: &str, buffer: BatchBuffer) -> JsValue {
        let event = Event::Receipt(
            wacore::types::events::Receipt::builder()
                .source(wacore::types::message::MessageSource {
                    chat: "5511999@s.whatsapp.net".parse().expect("valid chat jid"),
                    sender: "5511999:9@s.whatsapp.net"
                        .parse()
                        .expect("valid sender jid"),
                    ..Default::default()
                })
                .message_ids(vec![id.into()])
                .timestamp(Default::default())
                .r#type(wacore::types::presence::ReceiptType::Delivered)
                .offline(false)
                .build(),
        );
        ReceiptWireBatch::with_encoder(|encoder| {
            encoder.begin();
            encoder.push(&event).expect("packs");
            encoder.finish(buffer).expect("crosses")
        })
    }

    /// A conforming host: it reads the batch and returns, keeping nothing.
    fn decoding_callback(seen: Rc<RefCell<Vec<String>>>) -> js_sys::Function {
        let closure = Closure::wrap(Box::new(move |batch: js_sys::Uint8Array| {
            seen.borrow_mut()
                .push(String::from_utf8_lossy(&batch.to_vec()).into_owned());
        }) as Box<dyn FnMut(js_sys::Uint8Array)>);
        closure.into_js_value().unchecked_into()
    }

    /// A host that keeps the typed array it was handed.
    fn retaining_callback(held: Rc<RefCell<Vec<js_sys::Uint8Array>>>) -> js_sys::Function {
        let closure = Closure::wrap(Box::new(move |batch: js_sys::Uint8Array| {
            held.borrow_mut().push(batch);
        }) as Box<dyn FnMut(js_sys::Uint8Array)>);
        closure.into_js_value().unchecked_into()
    }

    fn channel(
        copying: Option<js_sys::Function>,
        borrowing: Option<js_sys::Function>,
    ) -> PackedBatchChannelState {
        ReceiptWireBatch::with_encoder(|encoder| *encoder = ReceiptWireBatch::default());
        // Revocation is permanent and process-wide, so a test that provokes one
        // would otherwise decide the outcome of every test after it.
        crate::wire_batch::reset_borrowed_batches();
        PackedBatchChannelState { copying, borrowing }
    }

    /// A second kind delivered by the same host, as `from_js` builds the
    /// receipt and server-ack pair.
    fn sibling_channel(borrowing: js_sys::Function) -> PackedBatchChannelState {
        PackedBatchChannelState {
            copying: None,
            borrowing: Some(borrowing),
        }
    }

    /// A callback that returns synchronously the first time and promise-like
    /// afterwards, counting its own calls so no shared global can shift it.
    fn conditionally_async_callback(async_from: u32, thenable: JsValue) -> js_sys::Function {
        let calls = Cell::new(0u32);
        let closure = Closure::wrap(Box::new(move |_: js_sys::Uint8Array| -> JsValue {
            calls.set(calls.get() + 1);
            if calls.get() >= async_from {
                thenable.clone()
            } else {
                JsValue::UNDEFINED
            }
        }) as Box<dyn FnMut(js_sys::Uint8Array) -> JsValue>);
        closure.into_js_value().unchecked_into()
    }

    fn deliver(channel: &PackedBatchChannelState, id: &str) -> JsValue {
        let batch = cross_receipt_batch(id, channel.buffer());
        channel
            .call(&JsValue::NULL, ReceiptWireBatch::KIND, &batch)
            .expect("the callback returned");
        batch
    }

    /// Each callback sees its own batch: the shared buffer is rewritten only
    /// after the previous call has returned.
    #[test]
    fn a_borrowing_host_sees_its_own_batch_inside_each_callback() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let channel = channel(None, Some(decoding_callback(seen.clone())));

        for id in ["RCPT-A", "RCPT-B"] {
            assert_eq!(channel.buffer(), BatchBuffer::Borrowed);
            deliver(&channel, id);
        }

        let seen = seen.borrow();
        assert!(seen[0].contains("RCPT-A") && !seen[0].contains("RCPT-B"));
        assert!(seen[1].contains("RCPT-B") && !seen[1].contains("RCPT-A"));
    }

    /// The default path is untouched: without the opt-in every batch keeps its
    /// own buffer, so a host may hold one for as long as it likes.
    #[test]
    fn a_host_that_did_not_opt_in_keeps_a_buffer_per_batch() {
        let held = Rc::new(RefCell::new(Vec::new()));
        let channel = channel(Some(retaining_callback(held.clone())), None);

        for id in ["RCPT-A", "RCPT-B"] {
            assert_eq!(channel.buffer(), BatchBuffer::Owned);
            deliver(&channel, id);
        }

        let held = held.borrow();
        assert!(
            !js_sys::Object::is(&held[0].buffer().into(), &held[1].buffer().into()),
            "the batches shared a buffer"
        );
        assert!(String::from_utf8_lossy(&held[0].to_vec()).contains("RCPT-A"));
    }

    /// A borrowing callback that returns a Promise has broken the contract. It
    /// is revoked before the shared buffer is written again, so the window it
    /// kept is still its own.
    #[test]
    fn a_borrowing_callback_that_returns_a_promise_is_revoked() {
        let channel = channel(
            None,
            Some(js_sys::Function::new_no_args("return Promise.resolve()")),
        );

        let first = deliver(&channel, "RCPT-A").unchecked_into::<js_sys::Uint8Array>();
        assert_eq!(channel.buffer(), BatchBuffer::Owned, "the borrow survived");

        let bytes = first.to_vec();
        deliver(&channel, "RCPT-B");
        assert_eq!(first.to_vec(), bytes, "the retained batch was rewritten");
    }

    /// The dangerous shape is the callback that only goes async down one of its
    /// branches: a contract check that ran once would clear it and then miss
    /// the branch that breaks it.
    #[test]
    fn a_borrowing_callback_that_turns_async_later_is_revoked_then() {
        let channel = channel(
            None,
            Some(conditionally_async_callback(
                2,
                js_sys::Promise::resolve(&JsValue::UNDEFINED).into(),
            )),
        );

        deliver(&channel, "RCPT-A");
        assert_eq!(
            channel.buffer(),
            BatchBuffer::Borrowed,
            "a synchronous return revoked the borrow"
        );

        let second = deliver(&channel, "RCPT-B").unchecked_into::<js_sys::Uint8Array>();
        assert_eq!(channel.buffer(), BatchBuffer::Owned, "the borrow survived");

        let bytes = second.to_vec();
        deliver(&channel, "RCPT-C");
        assert_eq!(second.to_vec(), bytes, "the retained batch was rewritten");
    }

    /// `instanceof Promise` is realm-local and blind to a bare thenable, and
    /// both defer the host's work exactly the way the borrow forbids.
    #[test]
    fn a_borrowing_callback_that_returns_a_bare_thenable_is_revoked() {
        let thenable = js_sys::Object::new();
        js_sys::Reflect::set(
            &thenable,
            &"then".into(),
            &js_sys::Function::new_no_args(""),
        )
        .expect("set then");
        let channel = channel(None, Some(conditionally_async_callback(1, thenable.into())));

        deliver(&channel, "RCPT-A");
        assert_eq!(channel.buffer(), BatchBuffer::Owned, "the borrow survived");
    }

    /// A callback that throws has not finished with its window either, and the
    /// dispatch loop logs the throw and carries on.
    #[test]
    fn a_borrowing_callback_that_throws_is_revoked() {
        let channel = channel(
            None,
            Some(js_sys::Function::new_no_args("throw new Error('boom')")),
        );

        let batch = cross_receipt_batch("RCPT-A", channel.buffer());
        let held = batch.clone().unchecked_into::<js_sys::Uint8Array>();
        channel
            .call(&JsValue::NULL, ReceiptWireBatch::KIND, &batch)
            .expect_err("the callback threw");
        assert_eq!(channel.buffer(), BatchBuffer::Owned, "the borrow survived");

        let bytes = held.to_vec();
        let next = cross_receipt_batch("RCPT-B", channel.buffer());
        let _ = channel.call(&JsValue::NULL, ReceiptWireBatch::KIND, &next);
        assert_eq!(held.to_vec(), bytes, "the retained batch was rewritten");
    }

    /// One buffer serves every packed kind and every client, so containment has
    /// to reach as far: a server-ack batch must not rewrite the window a
    /// revoked receipt callback kept.
    #[test]
    fn revoking_one_kind_stops_the_other_borrowing_too() {
        let receipts = channel(
            None,
            Some(js_sys::Function::new_no_args("return Promise.resolve()")),
        );
        let acks = sibling_channel(js_sys::Function::new_no_args(""));
        assert_eq!(acks.buffer(), BatchBuffer::Borrowed);

        let held = deliver(&receipts, "RCPT-A").unchecked_into::<js_sys::Uint8Array>();
        let bytes = held.to_vec();
        assert_eq!(
            acks.buffer(),
            BatchBuffer::Owned,
            "the sibling kept borrowing the buffer the revoked window is in"
        );

        deliver(&acks, "ACK-B");
        assert_eq!(held.to_vec(), bytes, "the retained batch was rewritten");
    }
}

/// Ordering, negotiation and coalescing behavior of the consumer loop, driven
/// end to end: synthetic events in, recorded host callbacks out.
#[cfg(test)]
mod event_delivery_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    use whatsapp_rust::wacore::types::events::{
        BatchOrigin, Connected, InboundMessage, MessageBatch, Receipt, ServerAck,
    };
    use whatsapp_rust::wacore::types::message::MessageInfo;
    use whatsapp_rust::wacore::types::presence::ReceiptType;
    use whatsapp_rust::waproto::whatsapp::Message;

    /// One observed host callback: the method name and the bytes it received.
    type Calls = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

    fn record(object: &js_sys::Object, method: &str, calls: &Calls) {
        let calls = calls.clone();
        let name = method.to_owned();
        let closure = Closure::wrap(Box::new(move |batch: JsValue| {
            // A host callback may re-enter the bridge, so the shared encoders
            // have to be empty here too, not only at the suspension points.
            debug_assert_encoders_empty();
            let bytes = batch
                .dyn_ref::<js_sys::Uint8Array>()
                .map(js_sys::Uint8Array::to_vec)
                .unwrap_or_default();
            calls.borrow_mut().push((name.clone(), bytes));
        }) as Box<dyn FnMut(JsValue)>);
        js_sys::Reflect::set(object, &method.into(), &closure.into_js_value())
            .expect("the callback object accepts a method");
    }

    /// Run the consumer loop over `events` against a host declaring `methods`,
    /// and return what it observed. Closing the channel ends the loop.
    async fn drive(methods: &[&str], events: Vec<Event>) -> Vec<(String, Vec<u8>)> {
        let object = js_sys::Object::new();
        let calls: Calls = Rc::new(RefCell::new(Vec::new()));
        for method in methods {
            record(&object, method, &calls);
        }
        drive_object(&object, events).await;
        calls.borrow().clone()
    }

    /// Run the consumer loop over `events` against an already built host.
    async fn drive_object(object: &js_sys::Object, events: Vec<Event>) {
        let callbacks =
            JsEventCallbacks::from_js(object.clone().into()).expect("the host shape parses");
        let (tx, rx) = async_channel::bounded::<Arc<Event>>(EVENT_CHANNEL_CAPACITY);
        for event in events {
            tx.try_send(Arc::new(event)).expect("the channel accepts");
        }
        tx.close();
        run_event_consumer(&callbacks, rx).await;
    }

    const LEGACY_HOST: &[&str] = &[
        EVENT_CALLBACK_METHOD,
        MESSAGE_BATCH_CALLBACK_METHOD,
        RECEIPT_BATCH_CALLBACK_METHOD,
        SERVER_ACK_BATCH_CALLBACK_METHOD,
    ];
    const ENVELOPE_HOST: &[&str] = &[
        EVENT_CALLBACK_METHOD,
        MESSAGE_BATCH_CALLBACK_METHOD,
        RECEIPT_BATCH_CALLBACK_METHOD,
        SERVER_ACK_BATCH_CALLBACK_METHOD,
        EVENT_BATCH_CALLBACK_METHOD,
    ];

    fn message(id: &str) -> Event {
        let mut info = MessageInfo::default();
        info.source.chat = "5511999@s.whatsapp.net".parse().expect("valid chat jid");
        info.source.sender = "5511999:9@s.whatsapp.net"
            .parse()
            .expect("valid sender jid");
        info.id = id.into();
        info.push_name = "Peer".into();
        let inbound = InboundMessage::builder()
            .message(Arc::new(Message::default()))
            .info(Arc::new(info))
            .build();
        Event::Messages(
            MessageBatch::builder()
                .messages(Arc::from(vec![inbound]))
                .origin(BatchOrigin::Live)
                .build(),
        )
    }

    fn receipt(id: &str) -> Event {
        let source = whatsapp_rust::wacore::types::message::MessageSource {
            chat: "5511999@s.whatsapp.net".parse().expect("valid chat jid"),
            sender: "5511999@s.whatsapp.net".parse().expect("valid sender jid"),
            ..Default::default()
        };
        Event::Receipt(
            Receipt::builder()
                .source(source)
                .message_ids(vec![id.into()])
                .timestamp(wacore::chrono::DateTime::from_timestamp(1, 0).expect("valid timestamp"))
                .r#type(ReceiptType::Delivered)
                .offline(false)
                .build(),
        )
    }

    fn ack(id: &str) -> Event {
        Event::ServerAck(ServerAck::builder().id(id.to_owned()).build())
    }

    fn connected() -> Event {
        Event::Connected(Connected::builder().build())
    }

    /// Ids travel as raw UTF-8 in every packed layout, so a batch's payload
    /// names the events it carries without decoding the layout here.
    fn carries(bytes: &[u8], id: &str) -> bool {
        String::from_utf8_lossy(bytes).contains(id)
    }

    fn names(calls: &[(String, Vec<u8>)]) -> Vec<&str> {
        calls.iter().map(|(name, _)| name.as_str()).collect()
    }

    /// Split an envelope the way `decodeEventWireEnvelope` does.
    fn segments(envelope: &[u8]) -> Vec<(u32, Vec<u8>)> {
        let u32_at = |at: usize| u32::from_le_bytes(envelope[at..at + 4].try_into().expect("4"));
        let count = u32_at(0) as usize;
        let mut out = Vec::with_capacity(count);
        let mut at = 8;
        for _ in 0..count {
            let kind = u32_at(at);
            let length = u32_at(at + 4) as usize;
            at += 8;
            out.push((kind, envelope[at..at + length].to_vec()));
            at = (at + length).next_multiple_of(8);
        }
        out
    }

    /// Happy path, legacy arm: the three batches a message turn produces still
    /// arrive one per crossing, in order.
    #[test]
    async fn a_host_without_the_envelope_keeps_one_batch_per_kind() {
        let calls = drive(LEGACY_HOST, vec![message("M1"), receipt("R1"), ack("A1")]).await;
        assert_eq!(
            names(&calls),
            [
                MESSAGE_BATCH_CALLBACK_METHOD,
                RECEIPT_BATCH_CALLBACK_METHOD,
                SERVER_ACK_BATCH_CALLBACK_METHOD
            ]
        );
        for (call, id) in calls.iter().zip(["M1", "R1", "A1"]) {
            assert!(carries(&call.1, id), "{} lost {id}", call.0);
        }
    }

    /// Happy path, new arm: the same turn costs one crossing, and each segment
    /// is byte for byte the batch the legacy arm received.
    #[test]
    async fn an_envelope_host_receives_the_same_batches_in_one_crossing() {
        let events = || vec![message("M1"), receipt("R1"), ack("A1")];
        let legacy = drive(LEGACY_HOST, events()).await;
        let coalesced = drive(ENVELOPE_HOST, events()).await;

        assert_eq!(names(&coalesced), [EVENT_BATCH_CALLBACK_METHOD]);
        let segments = segments(&coalesced[0].1);
        assert_eq!(
            segments.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
            [
                EVENT_SEGMENT_KIND_MESSAGE,
                EVENT_SEGMENT_KIND_RECEIPT,
                EVENT_SEGMENT_KIND_SERVER_ACK
            ]
        );
        for (segment, batch) in segments.iter().zip(legacy.iter()) {
            assert_eq!(segment.1, batch.1, "{} segment diverged", batch.0);
        }
    }

    /// The test the change stands on: a run where the batched kinds interleave
    /// with an event that is not batched at all must reach the host in exactly
    /// the order the events were dispatched.
    #[test]
    async fn coalescing_preserves_the_observed_order() {
        let events = vec![
            message("M1"),
            receipt("R1"),
            connected(),
            ack("A1"),
            message("M2"),
            receipt("R2"),
        ];
        let expected = ["M1", "R1", "connected", "A1", "M2", "R2"];

        for host in [LEGACY_HOST, ENVELOPE_HOST] {
            let calls = drive(host, events.clone()).await;
            let mut observed = Vec::new();
            for (name, bytes) in &calls {
                if name == EVENT_CALLBACK_METHOD {
                    observed.push("connected");
                    continue;
                }
                let batches: Vec<Vec<u8>> = if name == EVENT_BATCH_CALLBACK_METHOD {
                    segments(bytes).into_iter().map(|(_, b)| b).collect()
                } else {
                    vec![bytes.clone()]
                };
                for batch in batches {
                    let id = expected
                        .iter()
                        .find(|id| carries(&batch, id))
                        .expect("every batch names its event");
                    observed.push(id);
                }
            }
            assert_eq!(observed, expected, "order changed for a host");
        }
    }

    /// Negotiation: a host that declares the envelope but not every packed
    /// callback still gets every event, through the path it did declare.
    #[test]
    async fn a_partially_declared_host_loses_no_event() {
        let calls = drive(
            &[
                EVENT_CALLBACK_METHOD,
                MESSAGE_BATCH_CALLBACK_METHOD,
                EVENT_BATCH_CALLBACK_METHOD,
            ],
            vec![message("M1"), receipt("R1"), ack("A1")],
        )
        .await;
        assert_eq!(
            names(&calls),
            [
                MESSAGE_BATCH_CALLBACK_METHOD,
                EVENT_CALLBACK_METHOD,
                EVENT_CALLBACK_METHOD
            ],
            "undeclared kinds fall back to the single-event path"
        );
        assert!(carries(&calls[0].1, "M1"));
    }

    /// Degraded path: with nothing to coalesce, a batch crosses as the bare
    /// per-kind buffer it always was, so the envelope only ever appears when it
    /// saved a crossing.
    #[test]
    async fn a_lone_batch_still_crosses_through_its_own_callback() {
        let calls = drive(ENVELOPE_HOST, vec![receipt("R1")]).await;
        assert_eq!(names(&calls), [RECEIPT_BATCH_CALLBACK_METHOD]);
        assert!(carries(&calls[0].1, "R1"));
    }

    /// The borrow opt-in and the envelope compose. A lone batch still crosses in
    /// the shared buffer its own kind negotiated; a coalesced run gets a buffer
    /// of its own, because an envelope may carry a message segment whose decode
    /// returns views over it.
    #[test]
    async fn coalescing_keeps_a_lone_batch_borrowing_and_the_envelope_owned() {
        crate::wire_batch::reset_borrowed_batches();
        let held: Rc<RefCell<Vec<(String, js_sys::Uint8Array)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let object = js_sys::Object::new();
        for method in [
            EVENT_CALLBACK_METHOD,
            MESSAGE_BATCH_CALLBACK_METHOD,
            RECEIPT_BATCH_BORROWED_CALLBACK_METHOD,
            EVENT_BATCH_CALLBACK_METHOD,
        ] {
            let held = held.clone();
            let name = method.to_owned();
            let closure = Closure::wrap(Box::new(move |batch: js_sys::Uint8Array| {
                held.borrow_mut().push((name.clone(), batch));
            }) as Box<dyn FnMut(js_sys::Uint8Array)>);
            js_sys::Reflect::set(&object, &method.into(), &closure.into_js_value())
                .expect("the callback object accepts a method");
        }

        drive_object(&object, vec![receipt("RCPT-1")]).await;
        drive_object(&object, vec![message("MSG-1"), receipt("RCPT-2")]).await;
        let held = held.borrow().clone();
        assert_eq!(
            held.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            [
                RECEIPT_BATCH_BORROWED_CALLBACK_METHOD,
                EVENT_BATCH_CALLBACK_METHOD
            ]
        );
        let borrowed = held[0].1.to_vec();
        let envelope = held[1].1.to_vec();

        // One more borrowed batch of the same shape rewrites the shared buffer.
        drive_object(&object, vec![receipt("RCPT-3")]).await;
        assert_ne!(
            held[0].1.to_vec(),
            borrowed,
            "the lone batch did not come out of the shared buffer"
        );
        assert_eq!(
            held[1].1.to_vec(),
            envelope,
            "the envelope was rewritten by a later batch"
        );
    }

    /// The boundary's event ceiling bounds a whole crossing, not one segment of
    /// it: a mixed run must split across envelopes rather than hand the host a
    /// callback carrying more events than a single batch ever did.
    #[test]
    async fn an_envelope_never_carries_more_events_than_the_ceiling() {
        // Runs that each fit under the ceiling but together pass it.
        let mut events = Vec::new();
        for i in 0..EVENT_BATCH_CAPACITY - 1 {
            events.push(receipt(&format!("R{i}-")));
        }
        for i in 0..EVENT_BATCH_CAPACITY - 1 {
            events.push(ack(&format!("A{i}-")));
        }
        let total = events.len();
        let calls = drive(ENVELOPE_HOST, events).await;

        // Every packed layout opens with its record count as a u32.
        let records =
            |batch: &[u8]| u32::from_le_bytes(batch[..4].try_into().expect("4 bytes")) as usize;
        let mut delivered = 0;
        for (name, bytes) in &calls {
            let carried: usize = if name == EVENT_BATCH_CALLBACK_METHOD {
                segments(bytes).iter().map(|(_, b)| records(b)).sum()
            } else {
                records(bytes)
            };
            assert!(
                carried <= EVENT_BATCH_CAPACITY,
                "{name} carried {carried} events"
            );
            delivered += carried;
        }
        assert_eq!(delivered, total, "the run lost or duplicated an event");
    }

    /// Degraded path: a run past the boundary ceiling crosses in several
    /// pieces, and every event appears exactly once across them.
    #[test]
    async fn a_run_past_the_boundary_ceiling_splits_without_loss_or_duplication() {
        let total = EVENT_BATCH_CAPACITY * 2 + 3;
        let events: Vec<Event> = (0..total).map(|i| receipt(&format!("R{i}-"))).collect();
        let calls = drive(ENVELOPE_HOST, events).await;
        assert!(calls.len() > 1, "the run should not cross as one batch");

        for i in 0..total {
            let id = format!("R{i}-");
            let seen = calls
                .iter()
                .filter(|(_, bytes)| carries(bytes, &id))
                .count();
            assert_eq!(seen, 1, "{id} crossed {seen} times");
        }
    }
}
