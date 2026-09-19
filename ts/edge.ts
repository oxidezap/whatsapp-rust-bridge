/**
 * Edge entrypoint: same bridge, host-supplied WebAssembly bytes.
 *
 * The default entrypoint (`ts/index.ts`) reads `whatsapp_rust_bridge_bg.wasm`
 * off disk with `node:fs`, which does not exist on edge runtimes (workerd has
 * no `node:fs` at all; other hosts forbid filesystem reads). This module is
 * identical except for how the wasm bytes arrive: the host imports them with
 * whatever its platform provides and hands them to `initEdge`:
 *
 * ```ts
 * // Cloudflare Workers / workerd (static wasm import yields a Module)
 * import wasmModule from "./whatsapp_rust_bridge_bg.wasm";
 * import { initEdge } from "@oxidezap/whatsapp-rust-bridge/edge";
 * initEdge({ module: wasmModule });
 *
 * // Deno (bytes; needs --unstable-raw-imports or a readFile)
 * import wasmBytes from "./whatsapp_rust_bridge_bg.wasm" with { type: "bytes" };
 * import { initEdge } from "@oxidezap/whatsapp-rust-bridge/edge";
 * initEdge({ module: wasmBytes });
 *
 * // Node/Bun without the filesystem read
 * import { readFileSync } from "node:fs";
 * import { initEdge } from "@oxidezap/whatsapp-rust-bridge/edge";
 * initEdge({ module: readFileSync("./whatsapp_rust_bridge_bg.wasm") });
 * ```
 *
 * Why one entrypoint covers every host: `initSync` accepts either a compiled
 * `WebAssembly.Module` or raw bytes (`SyncInitInput`), and the two host
 * idioms produce exactly those — workerd/wrangler static imports compile to a
 * `WebAssembly.Module`, Deno/raw-import and bundler `?module` styles produce
 * bytes. Verified with evidence in `docs/edge-entrypoint.md`; there is no
 * workerd-only path because workerd needs nothing this module does not take.
 *
 * Two constraints the host must respect, both from the platform, not the bridge:
 * call `initEdge` once per isolate (a second call is a no-op returning the
 * existing instance), and create clients inside a request handler, not at
 * global scope — `crypto.getRandomValues` is unavailable during global-scope
 * evaluation on workerd, so client creation (uuid generation) panics there.
 */

// Re-export the generated WASM surface so feature-gated functions are exposed
// whenever their Rust feature is enabled. Explicit wrappers below take
// precedence for APIs whose public TypeScript signature needs refinement.
export * from "../pkg/whatsapp_rust_bridge.js";

// Pure-JS proto codec (bundled at build time, zero runtime deps for consumers)
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

// Packed wire-batch codecs (message metadata, receipts, server acks).
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

// Auto-assembled protobufjs-style namespace covering every ts-proto type.
// Lets `WAProto.X.encode(obj).finish()` and friends work for the full schema
// without a hand-maintained shim — see `proto-namespace.ts` for details.
export { proto } from "./proto-namespace";

import { initSync } from "../pkg/whatsapp_rust_bridge.js";
import type { InitOutput, SyncInitInput } from "../pkg/whatsapp_rust_bridge.js";

/**
 * Initialise the wasm module from host-supplied bytes or a compiled module.
 * Accepts the same shapes `initSync` does (`SyncInitInput`: bytes or
 * `WebAssembly.Module`); a second call in the same isolate returns the
 * existing instance without re-instantiating.
 */
export function initEdge(module: SyncInitInput): InitOutput {
  return initSync(module);
}

// initWasmEngine and createWhatsAppClient need explicit typing
// because they use skip_typescript in Rust for complex params.
import {
  initWasmEngine as _initWasmEngine,
  createWhatsAppClient as _createWhatsAppClient,
} from "../pkg/whatsapp_rust_bridge.js";
import type { WhatsAppEventHandler, JsTransportCallbacks, JsHttpClientConfig, JsStoreCallbacks, CacheConfig, ClientPolicies } from "../pkg/whatsapp_rust_bridge.js";
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
  dangerSkipCertChainVerify?: boolean | null,
  policies?: ClientPolicies | null,
) => Promise<WasmWhatsAppClient> = _createWhatsAppClient as any;
