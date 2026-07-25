import { readFileSync } from "node:fs";
import { initSync } from "../pkg/whatsapp_rust_bridge.js";

const wasmUrl = new URL("whatsapp_rust_bridge_bg.wasm", import.meta.url);
const wasmBytes = readFileSync(wasmUrl);
initSync({ module: wasmBytes });

// Re-export the generated WASM surface so feature-gated functions are exposed
// whenever their Rust feature is enabled. Explicit wrappers below take
// precedence for APIs whose public TypeScript signature needs refinement.
export * from "../pkg/whatsapp_rust_bridge.js";

// Pure-JS proto codec (bundled at build time, zero runtime deps for consumers)
export { encodeProto, decodeProto, decodeProtoBatch } from "./proto";
export { BinaryReader } from "./proto-reader";

// Packed wire-batch codecs (message metadata, receipts, server acks).
export {
  decodeMessageWireBatch,
  decodeReceiptWireBatch,
  decodeServerAckWireBatch,
  encodeMessageWireBatch,
  encodeReceiptWireBatch,
  encodeServerAckWireBatch,
  MESSAGE_WIRE_INFO_RECORD_WIDTH,
  type MessageWireBatchView,
  type MessageWireEntry,
  type MessageWireInfo,
  type PackedWireBatch,
  type ReceiptWireData,
  type ServerAckWireData,
  type WireJid,
} from "./wire-info";

// Auto-assembled protobufjs-style namespace covering every ts-proto type.
// Lets `WAProto.X.encode(obj).finish()` and friends work for the full schema
// without a hand-maintained shim — see `proto-namespace.ts` for details.
export { proto } from "./proto-namespace";

// initWasmEngine and createWhatsAppClient need explicit typing
// because they use skip_typescript in Rust for complex params.
import {
  initWasmEngine as _initWasmEngine,
  createWhatsAppClient as _createWhatsAppClient,
} from "../pkg/whatsapp_rust_bridge.js";
import type { WhatsAppEventHandler, JsTransportCallbacks, JsHttpClientConfig, JsStoreCallbacks, CacheConfig } from "../pkg/whatsapp_rust_bridge.js";
import type { WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";

export const initWasmEngine: (logger?: any, crypto?: any) => void = _initWasmEngine;

export const createWhatsAppClient: (
  transport: JsTransportCallbacks,
  httpClient: JsHttpClientConfig,
  onEvent?: WhatsAppEventHandler | null,
  store?: JsStoreCallbacks | null,
  cache?: CacheConfig | null,
  version?: readonly [number, number, number] | null,
  wantedPreKeyCount?: number | null,
) => Promise<WasmWhatsAppClient> = _createWhatsAppClient as any;
