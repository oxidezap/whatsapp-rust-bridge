/**
 * The one JS surface every entrypoint publishes.
 *
 * `ts/index.ts` (default: finds and loads the wasm itself) and `ts/host.ts`
 * (host-supplied wasm bytes) both re-export this module, so the API exists in
 * exactly one source. A method added here reaches both entrypoints; a typo in
 * one entrypoint cannot drift the other's client signature.
 */

export * from "../pkg/whatsapp_rust_bridge.js";

export {
  encodeProto,
  decodeProto,
  decodeProtoBatch,
  UnpairedSurrogateError,
  type ProtoDecodeReport,
} from "./proto";
export {
  BinaryReader,
  InvalidUtf8CountingReader,
  longToBigInt,
  type Int64,
  type Long,
} from "./proto-reader";

export {
  decodeEventWireEnvelope,
  decodeMessageWireBatch,
  decodeReceiptWireBatch,
  decodeServerAckWireBatch,
  encodeEventWireEnvelope,
  encodeMessageWireBatch,
  encodeReceiptWireBatch,
  encodeServerAckWireBatch,
  EVENT_SEGMENT_KIND_MESSAGE,
  EVENT_SEGMENT_KIND_RECEIPT,
  EVENT_SEGMENT_KIND_SERVER_ACK,
  MESSAGE_WIRE_INFO_RECORD_WIDTH,
  type EventWireSegment,
  type MessageWireBatchView,
  type MessageWireEntry,
  type MessageWireInfo,
  type PackedWireBatch,
  type ReceiptWireData,
  type ServerAckWireData,
  type WireJid,
} from "./wire-info";

export { proto } from "./proto-namespace";

export {
  buildRelayAnswerSdp,
  createRtcRelayTransportProvider,
  evaluateOutboundPacket,
  isRelayControlPacket,
  normalizeDtlsFingerprint,
  OutboundAuTracker,
  RELAY_DTLS_FINGERPRINT,
  RTP_PAYLOAD_TYPE_H264,
  shedBufferedPacket,
  type OutboundAuState,
  type RelayAnswerParts,
  type RtcRelayTransportOptions,
} from "./relay-transport";
export type { VoipBackendCallbacks, ClientExtensions } from "./voip-backend";
export type {
  VoipRelayConnection,
  VoipRelayConnectionEvents,
  VoipRelayEndpoint,
  VoipRelayTransport,
} from "./voip-relay-transport";

import {
  initWasmEngine as _initWasmEngine,
  createWhatsAppClient as _createWhatsAppClient,
} from "../pkg/whatsapp_rust_bridge.js";
import type { WhatsAppEventHandler, JsTransportCallbacks, JsHttpClientConfig, JsStoreCallbacks, CacheConfig, ClientPolicies } from "../pkg/whatsapp_rust_bridge.js";
import type { WasmCallHandle, WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";
import type { ClientExtensions } from "./voip-backend";

export type { WasmCallHandle };

// initWasmEngine and createWhatsAppClient need explicit typing
// because they use skip_typescript in Rust for complex params.
export const initWasmEngine: (logger?: any, crypto?: any) => void = _initWasmEngine;

/**
 * The factory promise is the initialization barrier. Once it resolves,
 * persistence, adapters, and the core client are ready; it does not imply a
 * socket, authentication, a started run loop, or completed history sync.
 */
export const createWhatsAppClient: (
  transport: JsTransportCallbacks,
  httpClient: JsHttpClientConfig,
  onEvent?: WhatsAppEventHandler | null,
  store?: JsStoreCallbacks | null,
  cache?: CacheConfig | null,
  version?: readonly [number, number, number] | null,
  wantedPreKeyCount?: number | null,
  dangerSkipCertChainVerify?: boolean | null,
  policies?: ClientPolicies | null,
  extensions?: ClientExtensions | null,
) => Promise<WasmWhatsAppClient> = _createWhatsAppClient as any;
