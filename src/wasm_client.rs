//! Full WhatsApp client running in WASM.
//!
//! Wraps `whatsapp_rust::Client` with JS-provided adapters for
//! transport (WebSocket), storage (InMemory/JS), and HTTP (fetch).

// The conversion helpers here are shared by the per-domain modules below, so
// turning a domain off leaves the ones only it called unused. That is the
// feature working, not a defect — but the allow is scoped to exactly the builds
// where it is expected, so the default build still reports a helper that has
// genuinely lost its last caller.
#![cfg_attr(
    not(all(
        feature = "client-business",
        feature = "client-chat-actions",
        feature = "client-contacts",
        feature = "client-groups",
        feature = "client-media",
        feature = "client-newsletter",
        feature = "client-signal",
    )),
    allow(dead_code)
)]

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
    /// Event names and the proto-field keys beside them, interned once per
    /// thread alongside the envelope keys in [`crate::js_keys`]. Bounded by the
    /// event enum.
    static INTERNED_NAMES: RefCell<HashMap<&'static str, JsValue>> =
        RefCell::new(HashMap::new());
}

fn interned(name: &'static str) -> JsValue {
    INTERNED_NAMES.with(|cache| {
        cache
            .borrow_mut()
            .entry(name)
            .or_insert_with(|| JsValue::from_str(name))
            .clone()
    })
}

fn make_js_event(event_type: &'static str, data: &JsValue) -> Result<JsValue, JsValue> {
    let event = js_sys::Object::new();
    js_keys::set(&event, &js_keys::EVENT_TYPE_KEY, &interned(event_type))?;
    js_keys::set(&event, &js_keys::EVENT_DATA_KEY, data)?;
    Ok(event.into())
}

macro_rules! bridge_events {
    (
        serialize {
            $( $variant:ident => $name:literal => $ts_type:literal ),* $(,)?
        }
        serialize_with_proto {
            $( $pvariant:ident => $pname:literal => $pts_type:literal => $pfield:ident ),* $(,)?
        }
        special {
            $( $xvariant:ident => $xname:literal => $xts:literal ),* $(,)?
        }
    ) => {
        // Generate WhatsAppEvent TypeScript type
        #[wasm_bindgen(typescript_custom_section)]
        const _TS_WHATSAPP_EVENT: &str = concat!(
            "export type WhatsAppEvent =\n",
            $( ev!($name, $ts_type), )*
            $( ev!($pname, $pts_type), )*
            $( ev!($xname, $xts), )*
            ";\n",
        );

        /// Every core `Event` variant this bridge carries across, by whichever
        /// path carries it.
        #[cfg(test)]
        const DISPATCHED_EVENT_VARIANTS: &[&str] = &[
            $( stringify!($variant), )*
            $( stringify!($pvariant), )*
            $( stringify!($xvariant), )*
        ];

        // Generate event_to_js dispatch (JS-specific, existing path)
        fn event_to_js(event: &Event) -> Result<JsValue, JsValue> {
            let (event_type, data) = match event {
                $( Event::$variant(data) => ($name, crate::proto::to_js_value(data)?), )*
                $( Event::$pvariant(data) => {
                    let value = crate::proto::to_js_value(data)?;
                    let proto = crate::camel_serializer::to_js_value_camel_preserve_top_level_presence(
                        &data.$pfield,
                    )?;
                    js_sys::Reflect::set(&value, &interned(stringify!($pfield)), &proto)?;
                    ($pname, value)
                } )*
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
        SelfPushNameUpdated      => "self_push_name_updated"        => "SelfPushNameUpdated",
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
        AppStateSyncFailed       => "app_state_sync_failed"         => "AppStateSyncFailed",
        ContactRemoved           => "contact_removed"               => "ContactRemoved",
        PairingQrCodesExhausted  => "pairing_qr_codes_exhausted"    => "PairingQrCodesExhausted",
        ClientExpirationChanged  => "client_expiration_changed"     => "ClientExpirationChanged",
    }
    // Events carrying a protobuf field beside their own. That field crosses in
    // the protobufjs shape its declaration names, keeping an explicit `false` or
    // `0` on the mutation itself — unpin and unarchive are that value.
    serialize_with_proto {
        // Variant                     => "js_name"                         => "TsDataType"                      => proto field
        ContactUpdate                  => "contact_update"                  => "ContactUpdate" => action,
        PinUpdate                      => "pin_update"                      => "PinUpdate" => action,
        MuteUpdate                     => "mute_update"                     => "MuteUpdate" => action,
        ArchiveUpdate                  => "archive_update"                  => "ArchiveUpdate" => action,
        StarUpdate                     => "star_update"                     => "StarUpdate" => action,
        MarkChatAsReadUpdate           => "mark_chat_as_read_update"        => "MarkChatAsReadUpdate" => action,
        DeleteChatUpdate               => "delete_chat_update"              => "DeleteChatUpdate" => action,
        ClearChatUpdate                => "clear_chat_update"               => "ClearChatUpdate" => action,
        UserStatusMuteUpdate           => "user_status_mute_update"         => "UserStatusMuteUpdate" => action,
        DeleteMessageForMeUpdate       => "delete_message_for_me_update"    => "DeleteMessageForMeUpdate" => action,
        LabelEditUpdate                => "label_edit_update"               => "LabelEditUpdate" => action,
        LabelAssociationUpdate         => "label_association_update"        => "LabelAssociationUpdate" => action,
        MessageLabelAssociationUpdate  => "message_label_association_update" => "MessageLabelAssociationUpdate" => action,
        QuickReplyUpdate               => "quick_reply_update"              => "QuickReplyUpdate" => action,
        DisableLinkPreviewsUpdate      => "disable_link_previews_update"    => "DisableLinkPreviewsUpdate" => action,
        CallLogSync                    => "call_log_sync"                   => "CallLogSync" => record,
    }
    special {
        // Variant                     => "js_name"                         => "TsDataType"
        Connected                      => "connected"                       => "Record<string, never>",
        Disconnected                   => "disconnected"                    => "Record<string, never>",
        PairingQrCode                  => "qr"                              => "{ code: string; timeout: number }",
        PairingCode                    => "pairing_code"                    => "{ code: string; timeout: number }",
        PairSuccess                    => "pair_success"                    => "{ id: string; lid: string; business_name: string; platform: string }",
        PairError                      => "pair_error"                      => "{ id: string; lid: string; business_name: string; platform: string; error: string }",
        LoggedOut                      => "logged_out"                      => "{ on_connect: boolean; reason: string }",
        Messages                       => "message"                         => "{ message: Record<string, unknown>; info: MessageInfo & { is_view_once: boolean } }",
        Notification                   => "notification"                    => "{ tag: string; attrs: Record<string, string>; content?: unknown }",
        StreamReplaced                 => "stream_replaced"                 => "Record<string, never>",
        QrScannedWithoutMultidevice    => "qr_scanned_without_multidevice"  => "Record<string, never>",
        ClientOutdated                 => "client_outdated"                 => "Record<string, never>",
        RawNode                        => "raw_node"                        => "{ tag: string; attrs: Record<string, string>; content?: unknown }",
        // The bridge itself never emits `message`/`history_sync` events — both
        // cross the boundary as wire batches (`onMessageBatch` /
        // `onHistorySyncBatch`). The union entries describe the host-side
        // reconstruction: hosts decode the wire payloads with their own codec
        // and rebuild these shapes for their downstream consumers.
        HistorySync                    => "history_sync"                    => "import('./proto-types').proto.IHistorySync & { syncType: number; chunkOrder?: number; progress?: number; peerDataRequestSessionId?: string }",
        // Special-cased for `backoff`: serializing a `Duration` emits
        // `{ secs, nanos }`, and this bridge crosses one as whole seconds.
        PairingCodeError               => "pairing_code_error"              => "PairingCodeError",
    }
}

/// The core `Event` variants this bridge deliberately does not carry across.
/// Every variant has to appear here or in `DISPATCHED_EVENT_VARIANTS`, so one
/// added upstream cannot be dropped in silence.
#[cfg(test)]
const UNDISPATCHED_EVENT_VARIANTS: &[(&str, &str)] = &[
    (
        "DecryptedPayload",
        "the payload is the bytes, and the bytes are #[serde(skip)] — what would \
         cross is a header with the plaintext missing. Emitted only while a \
         consumer holds a forwarding lease, which this bridge never takes.",
    ),
    (
        "SentFrame",
        "its one field is the marshaled stanza, also #[serde(skip)], so the event \
         would cross as an empty object. Lease-gated like DecryptedPayload.",
    ),
    (
        "RetiredPushNameUpdate",
        "retired upstream: nothing dispatches it and nothing can, and its payload \
         is now an empty struct. The variant survives only to hold its position \
         in the core's index-keyed Serialize format. The current push name is on \
         MessageInfo::push_name.",
    ),
    (
        "EncDecryptFailed",
        "every field crosses, but the core emits nothing while no consumer holds \
         acquire_enc_decrypt_failed_forwarding(). Publishing it would add an \
         event that can never fire; delivering it needs a lease toggle of the \
         kind setRawNodeForwarding is, which is a separate change.",
    ),
];

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
   *
   * Decode every batch, exactly once, in the order it arrives, before calling
   * back into the client: addresses and push names repeat, so a batch defines
   * them once and later batches reference the table the decoder is holding.
   * A batch skipped or decoded out of order leaves that table describing a
   * history the records do not have.
   *
   * The return type is `void` and TypeScript lets an `async` method satisfy
   * it, so this is checked rather than trusted: a callback that hands back
   * anything promise-like has not decoded inside its call, and the bridge
   * gives up the cross-batch table for the rest of the session — every batch
   * then carries every value it names, which decodes the same in any order.
   * Keeping the buffer is fine; decoding it later is what costs the table.
   *
   * The check runs after the batch has been handed over, so the batch that
   * revealed the violation is the one it cannot save: decoded after a later
   * one, it reads that one's table. Every batch after it is safe whenever it
   * is decoded.
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

/// A callback that handed back something promise-like has not decoded inside
/// its call: its decodes land later, in an order the writer cannot see. The
/// packed tables span batches and are only sound in delivery order, so this
/// gives them up rather than let one host read another batch's strings. It is
/// separate from the borrow contract — a copying callback is welcome to keep
/// the buffer, but not to decode it out of order.
///
/// This is reactive, and it cannot reach the batch already handed over: that
/// one went out with a header written before the violation was visible, and if
/// the host decodes it after a later batch it reads that batch's table. The
/// bound is one batch, the one the host mishandled, and every batch after it
/// is safe under any ordering — which is the best a check downstream of
/// delivery can do. Making the in-flight batch safe too would mean every batch
/// carrying every value always, which is the cost this whole change removes.
///
/// A callback that threw is handled by the caller: the batch may not have been
/// read at all, which is a roll, not a permanent revocation.
fn note_batch_deferred(kind: &str, result: &Result<JsValue, JsValue>) {
    if result.as_ref().is_ok_and(is_thenable) {
        crate::wire_batch::revoke_packed_tables();
        log::error!(
            "JS {kind} batch callback returned something promise-like; the cross-batch string table is off for the rest of the session"
        );
    }
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
        // Borrowing or not, a deferred decode reorders this kind's table.
        note_batch_deferred(kind, &result);
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
        let result = self
            .on_message_batch
            .as_ref()
            .expect("message callback checked before dispatch")
            .call1(&self.receiver, batch);
        note_batch_deferred("message", &result);
        result
    }

    fn supports_history_sync_batching(&self) -> bool {
        self.on_history_sync_batch.is_some()
    }

    fn supports_message_wire_batching(&self) -> bool {
        self.on_message_batch.is_some()
    }

    fn call_event_batch(&self, batch: &JsValue) -> Result<JsValue, JsValue> {
        debug_assert!(self.on_event_batch.is_some());
        let result = self
            .on_event_batch
            .as_ref()
            .expect("event-envelope callback checked before dispatch")
            .call1(&self.receiver, batch);
        note_batch_deferred("event envelope", &result);
        result
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
                        crate::wire_batch::invalidate_packed_tables();
                    }
                }
                Err(e) => {
                    log::warn!("{} wire batch materialization failed: {e:?}", B::KIND);
                    crate::wire_batch::invalidate_packed_tables();
                }
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
            crate::wire_batch::invalidate_packed_tables();
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
                // The batch crossed but nothing says the host read it, and the
                // encoders count a written batch as held. Take it back.
                crate::wire_batch::invalidate_packed_tables();
            }
        }
        Err(e) => {
            log::warn!("Message wire batch materialization failed: {e:?}");
            crate::wire_batch::invalidate_packed_tables();
        }
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
            js_sys::Reflect::set(
                &d,
                &"reason".into(),
                &connect_failure_reason_str(&lo.reason).into(),
            )?;
            ("logged_out", d.into())
        }
        Event::PairingCodeError(error) => {
            let d = js_sys::Object::new();
            if let Some(rejection) = error.rejection {
                js_sys::Reflect::set(&d, &"rejection".into(), &(rejection.code() as f64).into())?;
            }
            if let Some(backoff) = error.backoff {
                js_sys::Reflect::set(&d, &"backoff".into(), &(backoff.as_secs() as f64).into())?;
            }
            js_sys::Reflect::set(&d, &"error".into(), &error.error.as_str().into())?;
            ("pairing_code_error", d.into())
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

const TIMESTAMP_RANGE: &str = "must be a finite number of milliseconds inside the i64 range";

/// A millisecond instant the core takes as `i64`.
///
/// Not [`parse_timeout_ms`]: that is a duration, so it is unsigned and its own
/// range check applies. This is a point in time, and which instants are
/// meaningful is the core's rule to enforce — `mute_chat_until` already
/// rejects anything at or before the epoch. What the core cannot catch is the
/// cast: `as i64` saturates, so `Infinity` arrives as `i64::MAX` and passes
/// every check downstream of it.
fn parse_timestamp_ms(field: &'static str, value: f64) -> Result<i64, crate::errors::BridgeError> {
    // `i64::MIN as f64` and `i64::MAX as f64` are both exactly ±2^63, so the
    // upper comparison has to be strict where the lower one does not.
    if value.is_finite() && value >= i64::MIN as f64 && value < i64::MAX as f64 {
        Ok(value as i64)
    } else {
        Err(crate::errors::BridgeError::InvalidArgument {
            field: field.into(),
            reason: TIMESTAMP_RANGE.into(),
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
        client: CoreClient::new(client),
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

/// Home of [`CoreClient`], and the only place its inner client is nameable.
///
/// The domain modules are siblings of this one rather than descendants, so the
/// field below is out of their reach: `self.client.0` does not compile from
/// `wasm_client/*.rs`, and `online()` or `unwaited(…)` is the only way in.
mod core_client {
    use super::Arc;

    /// Why a call reaches the core without waiting for a reconnect in flight.
    ///
    /// A `#[wasm_bindgen]` method cannot get at the client without saying one of
    /// these or asking for [`CoreClient::online`], which is what keeps the next
    /// method added here from landing in a bucket nobody chose for it. The value is
    /// never read: it exists so the choice is written down where the call is, and
    /// so the set below can be found by name rather than kept in a list somewhere
    /// else that would drift.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Unwaited {
        /// Nothing crosses the wire, so there is no connection to wait for.
        Local,
        /// The core discards this on a lost connection rather than retrying it. A
        /// receipt, an ack or a presence held back until the next socket says
        /// something about a moment that has passed.
        ConnectionBound,
        /// The socket itself is the subject — establishing one, driving one, or
        /// reporting on one.
        ThisSocket,
        /// The operation re-drives itself, or is built so its caller can re-issue
        /// it. Waiting would sit in front of a retry that already exists.
        Redriven,
        /// An opaque node from the caller. The bridge cannot tell an IQ that
        /// survives a reconnect from an ack that does not, so it does not guess;
        /// `reachability()` and `waitUntilReachable()` put the choice with the
        /// caller, who knows what the node is.
        Opaque,
        /// The method returns before anything is sent, so there is no call to hold.
        /// Whatever it hands back reports its own failure.
        Deferred,
        /// The socket is needed only when a cache the bridge cannot see is
        /// cold, and the core decides that per call. Holding it would park an
        /// HTTP transfer that a warm cache serves with no socket at all.
        Cached,
    }

    /// The core client, reachable only by saying what this call is.
    ///
    /// [`online`](Self::online) holds the call while a reconnect is in flight;
    /// [`unwaited`](Self::unwaited) does not, and names why. There is no third way
    /// in and no plain field, so a method added later has to pick one.
    pub(crate) struct CoreClient(Arc<whatsapp_rust::Client>);

    impl CoreClient {
        pub(crate) fn new(client: Arc<whatsapp_rust::Client>) -> Self {
            Self(client)
        }
    }

    impl CoreClient {
        /// The client, once any reconnect in flight has landed.
        ///
        /// Only `Reconnecting` is waited out — the one state the core says
        /// comes back with nothing further from the caller. Every other
        /// answer falls straight through to the core, which reports what it always
        /// reported: a finished client fails now, a paused one fails now, and a
        /// client nothing is reading fails now. Waiting restores the ability to
        /// ask, never the request that was refused, so nothing is re-sent here.
        ///
        /// Connected, this is a few relaxed loads and a branch. Nothing is
        /// allocated, no boundary is crossed, and the future that does the waiting
        /// is built only once one is needed.
        ///
        /// A parked call cannot be withdrawn. wasm-bindgen drives an exported
        /// async method to completion whether or not JS still holds its
        /// promise, so racing that promise against a deadline bounds the
        /// host's waiting and not the work: the call still goes out when the
        /// reconnect lands. A host that needs a bound on the work itself reads
        /// `reachability()` first, or races `waitUntilReachable()`, and only
        /// then decides to issue the call.
        #[inline]
        pub(crate) async fn online(&self) -> &Arc<whatsapp_rust::Client> {
            if self.0.reachability().recovers_on_its_own() {
                self.park().await;
            }
            &self.0
        }

        /// The client as it is right now.
        #[inline]
        pub(crate) fn unwaited(&self, _why: Unwaited) -> &Arc<whatsapp_rust::Client> {
            &self.0
        }

        /// What work handed to the client right now can expect.
        pub(crate) fn reachability(&self) -> crate::result_types::Reachability {
            self.0.reachability().into()
        }

        /// Wait out a reconnect, and report whatever ended the wait.
        pub(crate) async fn wait_until_reachable(&self) -> crate::result_types::Reachability {
            self.0.wait_until_reachable().await.into()
        }

        /// Off the hot path in its own future, so the check above costs one branch
        /// in each of the callers rather than a wait's worth of state machine.
        #[cold]
        #[inline(never)]
        fn park(&self) -> std::pin::Pin<Box<dyn core::future::Future<Output = ()> + '_>> {
            Box::pin(async move {
                let _ = self.0.wait_until_reachable().await;
            })
        }
    }
}

pub(crate) use core_client::{CoreClient, Unwaited};

/// Opaque handle to the WhatsApp client.
#[wasm_bindgen]
pub struct WasmWhatsAppClient {
    client: CoreClient,
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

// The exported surface is split across per-domain child modules, each with
// its own `#[wasm_bindgen] impl WasmWhatsAppClient` block. wasm-bindgen
// accepts several impl blocks for one type, and a child module reaches this
// module's private fields and helpers, so the split costs no visibility.
// Each optional one is a Cargo feature, on by default; connection and
// messaging are not optional. See `[features]` in Cargo.toml.
#[cfg(feature = "client-business")]
mod business;
#[cfg(feature = "client-chat-actions")]
mod chat_actions;
mod connection;
#[cfg(feature = "client-contacts")]
mod contacts;
#[cfg(feature = "client-groups")]
mod groups;
#[cfg(feature = "client-media")]
mod media;
mod messaging;
#[cfg(feature = "client-newsletter")]
mod newsletter;
#[cfg(feature = "client-signal")]
mod signal;

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
        self.client
            .unwaited(Unwaited::ThisSocket)
            .signal_shutdown_sync();

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
        let client = self.client.unwaited(Unwaited::ThisSocket).clone();
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

/// Parse what a participant update needs, so its caller can reject a bad JID
/// before deciding whether to wait for a connection.
fn participants_update_input(
    jid: &str,
    participants: &[String],
    action: crate::result_types::GroupParticipantAction,
) -> Result<(wacore_binary::jid::Jid, Vec<wacore_binary::jid::Jid>), crate::errors::BridgeError> {
    if matches!(action, crate::result_types::GroupParticipantAction::Modify) {
        return Err(crate::errors::BridgeError::InvalidArgument {
            field: "action".into(),
            reason: "modify represents a received participant identity change and cannot be sent"
                .into(),
        });
    }
    let group_jid = parse_jid(jid)?;
    let participant_jids = participants
        .iter()
        .map(|participant| parse_jid(participant))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((group_jid, participant_jids))
}

async fn participants_update(
    client: &whatsapp_rust::Client,
    group_jid: wacore_binary::jid::Jid,
    participant_jids: Vec<wacore_binary::jid::Jid>,
    action: crate::result_types::GroupParticipantAction,
    include_linked_groups_on_remove: bool,
) -> Result<Vec<crate::result_types::ParticipantChangeResult>, crate::errors::BridgeError> {
    use crate::result_types::GroupParticipantAction;

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
        // `participants_update_input` rejects this above the gate, so a caller
        // hears it at once rather than after a reconnect. Kept as a real error
        // rather than a trap: a trap does not cross as a `WhatsAppError` with
        // a `.kind`, and a later caller reaching here without the helper would
        // get no usable failure at all.
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

/// Deserialize a typed parameter, naming the field when the shape is wrong.
///
/// Taking these as `JsValue` rather than through `#[tsify(from_wasm_abi)]` is
/// what makes a bad argument a rejection: tsify's generated `FromWasmAbi`
/// throws from inside the async shim, where the throw escapes as an uncaught
/// exception and leaves the promise pending for good. The declared TypeScript
/// type is preserved by `unchecked_param_type` on the parameter.
fn from_js_input<T: serde::de::DeserializeOwned>(
    field: &'static str,
    value: JsValue,
) -> Result<T, crate::errors::BridgeError> {
    serde_wasm_bindgen::from_value(value).map_err(|e| crate::errors::BridgeError::InvalidArgument {
        field: field.into(),
        reason: e.to_string(),
    })
}

/// Take a parameter declared as an imported JS class, naming the field when it
/// cannot do what this code is about to ask of it.
///
/// The same trap [`from_js_input`] exists for, reached the other way:
/// wasm-bindgen casts an imported type *unchecked*, so a plain object is
/// accepted at the boundary and fails much deeper, where the throw escapes the
/// async shim as an uncaught exception and the promise stays pending for good.
/// `unchecked_param_type` keeps the declared TypeScript type.
///
/// The test is for `method` rather than for `instanceof`, because `instanceof`
/// asks the wrong question. A stream from an iframe, a worker, or a WHATWG
/// ponyfill is a working stream carrying a different realm's constructor, and
/// brand-checking would start rejecting arguments that used to run fine.
///
/// And it *calls* the method rather than merely finding it, because a plain
/// object can borrow `ReadableStream.prototype.getReader` and satisfy any
/// check short of running it. Through `Reflect` the throw is a `Result`;
/// through the stream machinery it would be the uncaught exception again.
fn from_js_class<T: JsCast>(
    field: &'static str,
    expected: &str,
    method: &str,
    value: JsValue,
) -> Result<T, crate::errors::BridgeError> {
    let refuse = |detail: String| crate::errors::BridgeError::InvalidArgument {
        field: field.into(),
        reason: format!("must be a usable {expected}: {detail}"),
    };

    let lock = call_no_args(&value, method).map_err(&refuse)?;
    // Hand it straight back. The stream has to reach `wasm_streams` unlocked,
    // and one whose lock will not release is no more usable than one that
    // could not produce it.
    call_no_args(&lock, "releaseLock").map_err(&refuse)?;

    Ok(value.unchecked_into())
}

/// Call `target[method]()`, describing rather than raising anything that goes
/// wrong. Both halves are catchable: `Reflect::get` errors on a primitive or on
/// `null` instead of panicking, and `call0` is a `catch` binding, so a throw
/// comes back as a value instead of unwinding out of the shim.
fn call_no_args(target: &JsValue, method: &str) -> Result<JsValue, String> {
    let member = js_sys::Reflect::get(target, &JsValue::from_str(method))
        .ok()
        .filter(JsValue::is_function)
        .ok_or_else(|| format!("nothing callable at .{method}()"))?;

    member
        .unchecked_into::<js_sys::Function<fn() -> JsValue>>()
        .call0(target)
        .map_err(|thrown| {
            let detail = thrown
                .dyn_ref::<js_sys::Error>()
                .map(|error| String::from(error.message()))
                .unwrap_or_else(|| format!("{thrown:?}"));
            format!(".{method}() threw {detail}")
        })
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

/// Decode and parse what a status send needs, so its caller can reject bad
/// input before deciding whether to wait for a connection.
fn status_message_input(
    bytes: &[u8],
    recipients: &[String],
) -> Result<(waproto::whatsapp::Message, Vec<wacore_binary::jid::Jid>), crate::errors::BridgeError>
{
    let msg = waproto::codec::message_decode(bytes)
        .map_err(|e| crate::errors::internal(format!("invalid message bytes: {e}")))?;
    let recipients = recipients
        .iter()
        .map(|jid| parse_jid(jid))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((msg, recipients))
}

async fn send_status_message_with_options(
    client: &whatsapp_rust::Client,
    msg: waproto::whatsapp::Message,
    recipients: Vec<wacore_binary::jid::Jid>,
    options: whatsapp_rust::StatusSendOptions,
) -> Result<String, crate::errors::BridgeError> {
    let result = client.status().send_raw(msg, &recipients, options).await?;
    Ok(result.message_id)
}

fn admin_profile_to_result(
    profile: &whatsapp_rust::features::NewsletterAdminProfile,
) -> crate::result_types::NewsletterAdminProfileResult {
    crate::result_types::NewsletterAdminProfileResult {
        id: profile.id.clone(),
        name: profile.name.clone(),
        picture_id: profile.picture_id.clone(),
        picture_direct_path: profile.picture_direct_path.clone(),
    }
}

/// The JS spelling of a core enum, written down here rather than taken from
/// `Debug`.
///
/// `Debug` is not a stable API. Renaming a variant upstream is an ordinary
/// refactor there and neither crate would fail to build, but the string a host
/// switches on would have changed. Each mapping below emits exactly what the
/// surface emits today, so this is a change of source rather than of value.
fn newsletter_verification_str(v: &whatsapp_rust::features::NewsletterVerification) -> String {
    use whatsapp_rust::features::NewsletterVerification as V;
    match v {
        V::Verified => "Verified".into(),
        V::Unverified => "Unverified".into(),
        // `#[non_exhaustive]` forces a wildcard. `Debug` keeps a variant added
        // upstream identifiable rather than folding it into a name above.
        other => format!("{other:?}"),
    }
}

/// See [`newsletter_verification_str`].
fn newsletter_state_str(s: &whatsapp_rust::features::NewsletterState) -> String {
    use whatsapp_rust::features::NewsletterState as S;
    match s {
        S::Active => "Active".into(),
        S::Suspended => "Suspended".into(),
        S::Geosuspended => "Geosuspended".into(),
        other => format!("{other:?}"),
    }
}

/// See [`newsletter_verification_str`].
fn newsletter_role_str(r: &whatsapp_rust::features::NewsletterRole) -> String {
    use whatsapp_rust::features::NewsletterRole as R;
    match r {
        R::Owner => "Owner".into(),
        R::Admin => "Admin".into(),
        R::Subscriber => "Subscriber".into(),
        R::Guest => "Guest".into(),
        other => format!("{other:?}"),
    }
}

/// See [`newsletter_verification_str`].
///
/// This one is matched whole: the core enum is not `#[non_exhaustive]`, so a
/// variant added upstream stops the build here and has to be given a string
/// deliberately.
fn connect_failure_reason_str(r: &wacore::types::events::ConnectFailureReason) -> String {
    use wacore::types::events::ConnectFailureReason as R;
    match r {
        R::Generic => "Generic".into(),
        R::LoggedOut => "LoggedOut".into(),
        R::TempBanned => "TempBanned".into(),
        R::AccountLocked => "AccountLocked".into(),
        R::UnknownLogout => "UnknownLogout".into(),
        R::ClientOutdated => "ClientOutdated".into(),
        R::BadUserAgent => "BadUserAgent".into(),
        R::CatExpired => "CatExpired".into(),
        R::CatInvalid => "CatInvalid".into(),
        R::NotFound => "NotFound".into(),
        R::ClientUnknown => "ClientUnknown".into(),
        R::InternalServerError => "InternalServerError".into(),
        R::Experimental => "Experimental".into(),
        R::ServiceUnavailable => "ServiceUnavailable".into(),
        // The core's own fallback: the code it could not name, carried through.
        R::Unknown(code) => format!("Unknown({code})"),
    }
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
        verification: newsletter_verification_str(&meta.verification),
        state: newsletter_state_str(&meta.state),
        picture_url: meta.picture_url.clone(),
        preview_url: meta.preview_url.clone(),
        invite_code: meta.invite_code.clone(),
        role: meta.role.as_ref().map(newsletter_role_str),
        creation_time: meta.creation_time.map(|v| v as f64),
    }
}

#[cfg(test)]
mod enum_string_tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// The strings are the contract, so they are written out here rather than
    /// derived. A test that recomputed them from the same source would pass
    /// through any rename, which is the failure these mappings exist to stop.
    #[test]
    fn every_variant_keeps_its_spelling() {
        use whatsapp_rust::features::{
            NewsletterRole as R, NewsletterState as S, NewsletterVerification as V,
        };

        assert_eq!(newsletter_verification_str(&V::Verified), "Verified");
        assert_eq!(newsletter_verification_str(&V::Unverified), "Unverified");

        assert_eq!(newsletter_state_str(&S::Active), "Active");
        assert_eq!(newsletter_state_str(&S::Suspended), "Suspended");
        assert_eq!(newsletter_state_str(&S::Geosuspended), "Geosuspended");

        assert_eq!(newsletter_role_str(&R::Owner), "Owner");
        assert_eq!(newsletter_role_str(&R::Admin), "Admin");
        assert_eq!(newsletter_role_str(&R::Subscriber), "Subscriber");
        assert_eq!(newsletter_role_str(&R::Guest), "Guest");
    }

    #[test]
    fn connect_failure_reasons_keep_their_spelling() {
        use wacore::types::events::ConnectFailureReason as R;

        for (reason, expected) in [
            (R::Generic, "Generic"),
            (R::LoggedOut, "LoggedOut"),
            (R::TempBanned, "TempBanned"),
            (R::AccountLocked, "AccountLocked"),
            (R::UnknownLogout, "UnknownLogout"),
            (R::ClientOutdated, "ClientOutdated"),
            (R::BadUserAgent, "BadUserAgent"),
            (R::CatExpired, "CatExpired"),
            (R::CatInvalid, "CatInvalid"),
            (R::NotFound, "NotFound"),
            (R::ClientUnknown, "ClientUnknown"),
            (R::InternalServerError, "InternalServerError"),
            (R::Experimental, "Experimental"),
            (R::ServiceUnavailable, "ServiceUnavailable"),
        ] {
            assert_eq!(connect_failure_reason_str(&reason), expected);
        }
    }

    /// The code the core could not name still has to reach the host: a
    /// failure it cannot identify is one it cannot report.
    #[test]
    fn an_unnamed_failure_carries_its_code() {
        use wacore::types::events::ConnectFailureReason as R;

        assert_eq!(connect_failure_reason_str(&R::Unknown(499)), "Unknown(499)");
    }
}

/// Money crosses as a string: `amount_1000` is an i64 and a JS number is exact
/// only below 2^53, so a large order would come out silently wrong.
fn price_to_result(price: &whatsapp_rust::features::Price) -> crate::result_types::PriceResult {
    crate::result_types::PriceResult {
        amount_1000: price.amount_1000.to_string(),
        currency: price.currency.clone(),
    }
}

fn product_image_to_result(
    image: &whatsapp_rust::features::ProductImage,
) -> crate::result_types::ProductImageResult {
    crate::result_types::ProductImageResult {
        id: image.id.clone(),
        request_image_url: image.request_image_url.clone(),
        original_image_url: image.original_image_url.clone(),
    }
}

fn product_to_result(
    product: &whatsapp_rust::features::Product,
) -> crate::result_types::ProductResult {
    crate::result_types::ProductResult {
        id: product.id.clone(),
        retailer_id: product.retailer_id.clone(),
        name: product.name.clone(),
        description: product.description.clone(),
        url: product.url.clone(),
        shimmed_url: product.shimmed_url.clone(),
        price: product.price.as_ref().map(price_to_result),
        sale_price: product
            .sale_price
            .as_ref()
            .map(|sale| crate::result_types::SalePriceResult {
                price: price_to_result(&sale.price),
                start_date: sale.start_date.clone(),
                end_date: sale.end_date.clone(),
            }),
        is_hidden: product.is_hidden,
        is_sanctioned: product.is_sanctioned,
        max_available: product.max_available.map(|v| v as f64),
        availability: product.availability.as_ref().map(|a| a.as_str().to_owned()),
        review_status: product.review_status.clone(),
        can_appeal: product.can_appeal,
        belongs_to: product.belongs_to,
        images: product.images.iter().map(product_image_to_result).collect(),
        videos: product
            .videos
            .iter()
            .map(|video| crate::result_types::ProductVideoResult {
                id: video.id.clone(),
                original_video_url: video.original_video_url.clone(),
                thumbnail_url: video.thumbnail_url.clone(),
            })
            .collect(),
        compliance_category: product.compliance_category.clone(),
        country_code_origin: product.country_code_origin.clone(),
        importer_name: product.importer_name.clone(),
        importer_address: product.importer_address.as_ref().map(|address| {
            crate::result_types::ImporterAddressResult {
                street1: address.street1.clone(),
                street2: address.street2.clone(),
                city: address.city.clone(),
                region: address.region.clone(),
                postal_code: address.postal_code.clone(),
                country_code: address.country_code.clone(),
            }
        }),
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

    /// A `void` callback that hands back a promise decodes later, in an order
    /// the writer cannot see. TypeScript allows it — an `async` method
    /// satisfies a `void` signature — so the tables have to notice, whether or
    /// not the callback borrows. Without this the message path returns another
    /// batch's chat and push name, silently.
    #[test]
    fn a_batch_callback_that_defers_gives_up_the_cross_batch_tables() {
        crate::wire_batch::reset_packed_tables();

        note_batch_deferred("message", &Ok(JsValue::UNDEFINED));
        assert!(
            !crate::wire_batch::packed_tables_revoked(),
            "a conforming callback gave up the tables"
        );

        // A throw is the caller's business — the batch may not have been read
        // at all, which is a roll, not a permanent revocation.
        note_batch_deferred("message", &Err(JsValue::from_str("boom")));
        assert!(!crate::wire_batch::packed_tables_revoked());

        // Bare thenable, not `instanceof Promise`: an `async` method satisfies
        // a `void` signature in TypeScript, and so does anything with a `then`.
        let thenable = js_sys::Object::new();
        js_sys::Reflect::set(
            &thenable,
            &"then".into(),
            &js_sys::Function::new_no_args(""),
        )
        .expect("set then");
        note_batch_deferred("message", &Ok(thenable.into()));
        assert!(crate::wire_batch::packed_tables_revoked());

        crate::wire_batch::reset_packed_tables();
        MessageWireBatch::with_encoder(|encoder| *encoder = MessageWireBatch::default());
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
    pub(super) async fn drive_object(object: &js_sys::Object, events: Vec<Event>) {
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

/// One test per event this bridge dispatches out of the core's `Event`, driven
/// end to end: a synthetic core event in, the `onEvent` payload the host
/// received out.
#[cfg(test)]
mod dispatched_event_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen_test::wasm_bindgen_test as test;
    use whatsapp_rust::wacore::chrono::{DateTime, Utc};
    use whatsapp_rust::wacore::pair_code::PairCodeRejection;
    use whatsapp_rust::wacore::types::events::{
        AppStateSyncFailed, ArchiveUpdate, CallLogSync, ClientExpirationChanged, ContactRemoved,
        ContactUpdate, DecryptFailMode, DisableLinkPreviewsUpdate, MessageLabelAssociationUpdate,
        MuteUpdate, PairingCodeError, PairingQrCodesExhausted, PinUpdate, QuickReplyUpdate,
        UnavailableType, UndecryptableMessage,
    };
    use whatsapp_rust::wacore::types::message::{
        EncMediaType, MessageInfo, PollType, StanzaMessageType,
    };
    use whatsapp_rust::waproto::whatsapp::sync_action_value::{
        ArchiveChatAction, ContactAction, LabelAssociationAction, MuteAction, PinAction,
        PrivacySettingDisableLinkPreviewsAction, QuickReplyAction, SyncActionMessage,
        SyncActionMessageRange,
    };
    use whatsapp_rust::waproto::whatsapp::{CallLogRecord, MessageKey};

    /// The `{ type, data }` pairs an `onEvent` host was handed.
    type Seen = Rc<RefCell<Vec<(String, JsValue)>>>;

    /// Records the whole `{ type, data }`, not the packed bytes
    /// `event_delivery_tests` reads — these events have no packed form.
    fn host(seen: &Seen) -> js_sys::Object {
        let object = js_sys::Object::new();
        let seen = seen.clone();
        let closure = Closure::wrap(Box::new(move |event: JsValue| {
            let name = field(&event, "type").as_string().expect("type is a string");
            seen.borrow_mut().push((name, field(&event, "data")));
        }) as Box<dyn FnMut(JsValue)>);
        js_sys::Reflect::set(
            &object,
            &EVENT_CALLBACK_METHOD.into(),
            &closure.into_js_value(),
        )
        .expect("the host accepts onEvent");
        object
    }

    /// Run one event through the consumer loop and return what `onEvent` saw.
    async fn deliver(event: Event) -> (String, JsValue) {
        let seen: Seen = Rc::new(RefCell::new(Vec::new()));
        super::event_delivery_tests::drive_object(&host(&seen), vec![event]).await;
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1, "the host was not handed exactly one event");
        seen[0].clone()
    }

    fn field(data: &JsValue, key: &str) -> JsValue {
        js_sys::Reflect::get(data, &key.into()).expect("the key reads")
    }

    fn strings(value: &JsValue) -> Vec<String> {
        js_sys::Array::from(value)
            .iter()
            .map(|entry| entry.as_string().expect("an array of strings"))
            .collect()
    }

    fn jid(raw: &str) -> Jid {
        raw.parse().expect("valid jid")
    }

    fn timestamp() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("valid timestamp")
    }

    /// Load-bearing since #1291: the core announces the connection and reports
    /// what the critical sync left behind here.
    #[test]
    async fn an_app_state_sync_failed_carries_all_four_fields() {
        let (name, data) = deliver(Event::AppStateSyncFailed(
            AppStateSyncFailed::builder()
                .fatal(vec!["critical_block".to_owned()])
                .retryable(vec!["regular_high".to_owned()])
                .skipped(vec!["regular_low".to_owned()])
                .connected(true)
                .build(),
        ))
        .await;

        assert_eq!(name, "app_state_sync_failed");
        assert_eq!(strings(&field(&data, "fatal")), ["critical_block"]);
        assert_eq!(strings(&field(&data, "retryable")), ["regular_high"]);
        assert_eq!(strings(&field(&data, "skipped")), ["regular_low"]);
        assert_eq!(field(&data, "connected").as_bool(), Some(true));
    }

    #[test]
    async fn a_contact_removed_carries_the_contact_that_is_gone() {
        let (name, data) = deliver(Event::ContactRemoved(
            ContactRemoved::builder()
                .jid(jid("5511999@s.whatsapp.net"))
                .timestamp(timestamp())
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "contact_removed");
        assert_eq!(
            field(&field(&data, "jid"), "user").as_string().as_deref(),
            Some("5511999")
        );
        assert_eq!(field(&data, "from_full_sync").as_bool(), Some(false));
        // A `DateTime<Utc>` with no serde attribute crosses as chrono's RFC 3339
        // string, the same as every app-state event already exposed.
        assert_eq!(
            field(&data, "timestamp").as_string().as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
    }

    #[test]
    async fn a_disable_link_previews_update_carries_the_setting_and_its_action() {
        let (name, data) = deliver(Event::DisableLinkPreviewsUpdate(
            DisableLinkPreviewsUpdate::builder()
                .previews_disabled(true)
                .timestamp(timestamp())
                .action(Box::new(PrivacySettingDisableLinkPreviewsAction {
                    is_previews_disabled: Some(true),
                }))
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "disable_link_previews_update");
        assert_eq!(field(&data, "previews_disabled").as_bool(), Some(true));
        assert_eq!(
            field(&field(&data, "action"), "isPreviewsDisabled").as_bool(),
            Some(true)
        );
    }

    /// The record is the whole event: a call placed on the phone puts nothing on
    /// this socket, so no other event can see it.
    #[test]
    async fn a_call_log_sync_carries_the_record_and_who_placed_the_call() {
        let (name, data) = deliver(Event::CallLogSync(
            CallLogSync::builder()
                .call_creator_jid(jid("5511999@s.whatsapp.net"))
                .call_id("CALL-1".to_owned())
                .from_me(true)
                .timestamp(timestamp())
                .record(Box::new(CallLogRecord {
                    is_video: Some(true),
                    start_time: Some(1_700_000_000),
                    ..Default::default()
                }))
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "call_log_sync");
        assert_eq!(
            field(&data, "call_id").as_string().as_deref(),
            Some("CALL-1")
        );
        assert_eq!(field(&data, "from_me").as_bool(), Some(true));
        assert_eq!(
            field(&field(&data, "call_creator_jid"), "user")
                .as_string()
                .as_deref(),
            Some("5511999")
        );
        let record = field(&data, "record");
        assert_eq!(field(&record, "isVideo").as_bool(), Some(true));
        assert_eq!(
            field(&field(&record, "startTime"), "low").as_f64(),
            Some(1_700_000_000.0)
        );
    }

    /// An unpin is `pinned: false`, and an unarchive is `archived: false`: the
    /// value carries the transition, so a proto default that the wire actually
    /// set has to survive the crossing rather than be skipped as a default.
    #[test]
    async fn a_mutation_that_undoes_something_keeps_its_explicit_false() {
        let (name, data) = deliver(Event::PinUpdate(
            PinUpdate::builder()
                .jid(jid("5511999@s.whatsapp.net"))
                .timestamp(timestamp())
                .action(Box::new(PinAction {
                    pinned: Some(false),
                }))
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "pin_update");
        assert_eq!(
            field(&field(&data, "action"), "pinned").as_bool(),
            Some(false)
        );
    }

    /// A blank the wire supplied is not an absent field: `Some("")` is a name
    /// being cleared, and only a repeated field with no elements has no presence
    /// for protobuf to report.
    #[test]
    async fn a_mutation_keeps_a_blank_it_supplied_and_omits_what_it_never_set() {
        let (name, data) = deliver(Event::ContactUpdate(
            ContactUpdate::builder()
                .jid(jid("5511999@s.whatsapp.net"))
                .timestamp(timestamp())
                .action(Box::new(ContactAction {
                    full_name: Some(String::new()),
                    ..Default::default()
                }))
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "contact_update");
        let action = field(&data, "action");
        assert_eq!(field(&action, "fullName").as_string().as_deref(), Some(""));
        assert!(field(&action, "firstName").is_undefined());
    }

    /// A message range identifies the messages an action covers, and `fromMe`
    /// is half of a message's identity: a `false` the wire set has to survive
    /// the nesting, not just the mutation's own fields.
    #[test]
    async fn a_nested_message_keeps_the_presence_the_wire_gave_it() {
        let (name, data) = deliver(Event::ArchiveUpdate(
            ArchiveUpdate::builder()
                .jid(jid("5511999@s.whatsapp.net"))
                .timestamp(timestamp())
                .action(Box::new(ArchiveChatAction {
                    archived: Some(true),
                    message_range: Some(SyncActionMessageRange {
                        messages: vec![SyncActionMessage {
                            key: Some(MessageKey {
                                id: Some("MSG-1".to_owned()),
                                from_me: Some(false),
                                ..Default::default()
                            })
                            .into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    })
                    .into(),
                }))
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "archive_update");
        let messages = js_sys::Array::from(&field(
            &field(&field(&data, "action"), "messageRange"),
            "messages",
        ));
        let key = field(&messages.get(0), "key");
        assert_eq!(field(&key, "id").as_string().as_deref(), Some("MSG-1"));
        assert_eq!(field(&key, "fromMe").as_bool(), Some(false));
    }

    /// The events that already carried a proto mutation cross it in the same
    /// protobufjs shape, which is what their declarations have always named: a
    /// camelCase key, and the `Long` split for a 64-bit field.
    #[test]
    async fn an_already_exposed_action_crosses_in_the_shape_it_declares() {
        let (name, data) = deliver(Event::MuteUpdate(
            MuteUpdate::builder()
                .jid(jid("5511999@s.whatsapp.net"))
                .timestamp(timestamp())
                .action(Box::new(MuteAction {
                    muted: Some(true),
                    mute_end_timestamp: Some(1_700_000_000),
                    ..Default::default()
                }))
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "mute_update");
        let action = field(&data, "action");
        assert_eq!(field(&action, "muted").as_bool(), Some(true));
        let end = field(&action, "muteEndTimestamp");
        assert_eq!(field(&end, "low").as_f64(), Some(1_700_000_000.0));
        assert_eq!(field(&end, "high").as_f64(), Some(0.0));
        assert!(field(&action, "mute_end_timestamp").is_undefined());
    }

    #[test]
    async fn a_message_label_association_update_names_the_message_it_labelled() {
        let (name, data) = deliver(Event::MessageLabelAssociationUpdate(
            MessageLabelAssociationUpdate::builder()
                .label_id("7".to_owned())
                .chat_jid(jid("5511999@s.whatsapp.net"))
                .message_id("MSG-1".to_owned())
                .timestamp(timestamp())
                .action(Box::new(LabelAssociationAction {
                    labeled: Some(true),
                    ..Default::default()
                }))
                .from_full_sync(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "message_label_association_update");
        assert_eq!(field(&data, "label_id").as_string().as_deref(), Some("7"));
        assert_eq!(
            field(&data, "message_id").as_string().as_deref(),
            Some("MSG-1")
        );
        assert_eq!(
            field(&field(&data, "chat_jid"), "user")
                .as_string()
                .as_deref(),
            Some("5511999")
        );
        assert_eq!(
            field(&field(&data, "action"), "labeled").as_bool(),
            Some(true)
        );
    }

    #[test]
    async fn a_quick_reply_update_carries_the_shortcut_it_changed() {
        let (name, data) = deliver(Event::QuickReplyUpdate(
            QuickReplyUpdate::builder()
                .id("QR-1".to_owned())
                .timestamp(timestamp())
                .action(Box::new(QuickReplyAction {
                    shortcut: Some("/hello".to_owned()),
                    deleted: Some(false),
                    ..Default::default()
                }))
                .from_full_sync(true)
                .build(),
        ))
        .await;

        assert_eq!(name, "quick_reply_update");
        assert_eq!(field(&data, "id").as_string().as_deref(), Some("QR-1"));
        assert_eq!(field(&data, "from_full_sync").as_bool(), Some(true));
        let action = field(&data, "action");
        assert_eq!(
            field(&action, "shortcut").as_string().as_deref(),
            Some("/hello")
        );
        assert_eq!(field(&action, "deleted").as_bool(), Some(false));
        // A repeated field the mutation left unset has no presence to report,
        // so it stays absent rather than arriving as an empty array.
        assert!(field(&action, "keywords").is_undefined());
        assert!(field(&action, "associatedLabelIds").is_undefined());
    }

    #[test]
    async fn a_pairing_qr_codes_exhausted_says_whether_the_socket_is_still_up() {
        let (name, data) = deliver(Event::PairingQrCodesExhausted(
            PairingQrCodesExhausted::builder()
                .disconnected(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "pairing_qr_codes_exhausted");
        assert_eq!(field(&data, "disconnected").as_bool(), Some(false));
    }

    /// `backoff` crosses as whole seconds, the same unit `qr` and `pairing_code`
    /// already cross their `timeout` in.
    #[test]
    async fn a_pairing_code_error_carries_the_rejection_and_its_backoff() {
        let (name, data) = deliver(Event::PairingCodeError(
            PairingCodeError::builder()
                .rejection(PairCodeRejection::RateOverlimit)
                .backoff(Duration::from_secs(60))
                .error("rate-overlimit".to_owned())
                .build(),
        ))
        .await;

        assert_eq!(name, "pairing_code_error");
        assert_eq!(field(&data, "rejection").as_f64(), Some(429.0));
        assert_eq!(field(&data, "backoff").as_f64(), Some(60.0));
        assert_eq!(
            field(&data, "error").as_string().as_deref(),
            Some("rate-overlimit")
        );
    }

    /// An absent `backoff` stays absent: the bridge does not invent a zero for a
    /// hint the server did not send.
    #[test]
    async fn a_pairing_code_error_without_a_backoff_omits_it() {
        let (_, data) = deliver(Event::PairingCodeError(
            PairingCodeError::builder()
                .error("no connection".to_owned())
                .build(),
        ))
        .await;

        assert!(field(&data, "backoff").is_undefined());
        assert!(field(&data, "rejection").is_undefined());
    }

    /// A deadline the consumer is the only one who can act on: the core keeps
    /// connecting until the server refuses, so this event is the whole notice.
    #[test]
    async fn a_client_expiration_changed_carries_the_deadline_and_the_build() {
        let (name, data) = deliver(Event::ClientExpirationChanged(
            ClientExpirationChanged::builder()
                .expires_at(1_763_000_000)
                .version((2, 3000, 1044659339))
                .withdrawn(false)
                .build(),
        ))
        .await;

        assert_eq!(name, "client_expiration_changed");
        assert_eq!(field(&data, "expires_at").as_f64(), Some(1_763_000_000.0));
        assert_eq!(field(&data, "withdrawn").as_bool(), Some(false));
        let version = js_sys::Array::from(&field(&data, "version"));
        assert_eq!(version.get(0).as_f64(), Some(2.0));
        assert_eq!(version.get(1).as_f64(), Some(3000.0));
        assert_eq!(version.get(2).as_f64(), Some(1_044_659_339.0));
    }

    /// A withdrawal retracts the deadline, so there is no date to cross. The
    /// key is absent rather than zero: a zero reads as 1970, which is a deadline
    /// already past.
    #[test]
    async fn a_withdrawn_client_expiration_omits_the_deadline() {
        let (_, data) = deliver(Event::ClientExpirationChanged(
            ClientExpirationChanged::builder()
                .version((2, 3000, 1044659339))
                .withdrawn(true)
                .build(),
        ))
        .await;

        assert!(field(&data, "expires_at").is_undefined());
        assert_eq!(field(&data, "withdrawn").as_bool(), Some(true));
    }

    /// `type` and `media_type` were `String` and are now the wire vocabularies
    /// the core models. The strings a consumer switched on are unchanged; what
    /// changed is that a value outside the set no longer reads as one of them.
    #[test]
    async fn an_undecryptable_message_carries_the_envelope_type_and_the_enc_mediatype() {
        let mut info = MessageInfo::default();
        info.source.chat = jid("120363000000000001@g.us");
        info.source.sender = jid("5511999:7@s.whatsapp.net");
        info.id = "3EB0C1D2E3F4".into();
        info.push_name = "Alice".into();
        info.timestamp = timestamp();
        info.r#type = Some(StanzaMessageType::Poll);
        info.media_type = Some(EncMediaType::Ptt);
        info.meta_info.poll_type = Some(PollType::Vote);

        let (name, data) = deliver(Event::UndecryptableMessage(
            UndecryptableMessage::builder()
                .info(Arc::new(info))
                .is_unavailable(false)
                .unavailable_type(UnavailableType::Unknown)
                .decrypt_fail_mode(DecryptFailMode::Show)
                .build(),
        ))
        .await;

        assert_eq!(name, "undecryptable_message");
        let info = field(&data, "info");
        assert_eq!(field(&info, "type").as_string().as_deref(), Some("poll"));
        assert_eq!(
            field(&info, "media_type").as_string().as_deref(),
            Some("ptt")
        );
        assert_eq!(
            field(&field(&info, "meta_info"), "poll_type")
                .as_string()
                .as_deref(),
            Some("vote")
        );
    }

    /// Neither attribute is mandatory on the wire, and an absent one is now
    /// absent rather than `""` — the empty string was this bridge inventing a
    /// value for something the stanza never carried.
    #[test]
    async fn an_undecryptable_message_omits_an_envelope_type_the_stanza_did_not_carry() {
        let mut info = MessageInfo::default();
        info.source.chat = jid("5511999@s.whatsapp.net");
        info.source.sender = jid("5511999:7@s.whatsapp.net");
        info.id = "3EB0AAAABBBB".into();
        info.timestamp = timestamp();

        let (_, data) = deliver(Event::UndecryptableMessage(
            UndecryptableMessage::builder()
                .info(Arc::new(info))
                .is_unavailable(true)
                .unavailable_type(UnavailableType::ViewOnce)
                .decrypt_fail_mode(DecryptFailMode::Hide)
                .build(),
        ))
        .await;

        let info = field(&data, "info");
        assert!(field(&info, "type").is_undefined());
        assert!(field(&info, "media_type").is_undefined());
        assert!(field(&field(&info, "meta_info"), "poll_type").is_undefined());
        // The two enums the core rebuilt from the catalog keep their wire
        // spellings, which is the whole of what crosses here.
        assert_eq!(
            field(&data, "unavailable_type").as_string().as_deref(),
            Some("view_once")
        );
        assert_eq!(
            field(&data, "decrypt_fail_mode").as_string().as_deref(),
            Some("hide")
        );
    }
}

/// `Event` and `EventKind` are both `#[non_exhaustive]`, so no match here can be
/// exhaustive and the wildcard arm the core requires is what swallowed ten
/// variants. This measures the dispatch against the core's own list instead,
/// which `bun run gen:bridge-types` refreshes on every bump.
#[cfg(test)]
mod event_coverage_tests {
    use super::{DISPATCHED_EVENT_VARIANTS, UNDISPATCHED_EVENT_VARIANTS};
    use crate::generated_types::CORE_EVENT_VARIANTS;
    use wasm_bindgen_test::wasm_bindgen_test as test;

    fn is_excluded(variant: &str) -> bool {
        UNDISPATCHED_EVENT_VARIANTS
            .iter()
            .any(|(name, _)| *name == variant)
    }

    #[test]
    fn every_core_event_is_dispatched_or_excluded_on_the_record() {
        let unaccounted: Vec<&&str> = CORE_EVENT_VARIANTS
            .iter()
            .filter(|variant| {
                !DISPATCHED_EVENT_VARIANTS.contains(*variant) && !is_excluded(variant)
            })
            .collect();

        assert!(
            unaccounted.is_empty(),
            "the core dispatches {unaccounted:?}, which this bridge neither converts nor \
             lists as deliberately undispatched. Add an entry to `bridge_events!` or to \
             `UNDISPATCHED_EVENT_VARIANTS` with the reason."
        );
    }

    /// A name the core dropped or renamed leaves an entry here matching nothing,
    /// which would go on passing the check above while covering no variant.
    #[test]
    fn nothing_is_listed_that_the_core_no_longer_declares() {
        let stale: Vec<&str> = DISPATCHED_EVENT_VARIANTS
            .iter()
            .chain(UNDISPATCHED_EVENT_VARIANTS.iter().map(|(name, _)| name))
            .filter(|variant| !CORE_EVENT_VARIANTS.contains(variant))
            .copied()
            .collect();

        assert!(
            stale.is_empty(),
            "{stale:?} name no variant of the core's `Event`"
        );
    }
}
