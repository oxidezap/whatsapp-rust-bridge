import { createRequire } from "node:module";

import type {
  EnabledFeatures,
  PollVoterEntry,
  PollAggregateResult,
  WhatsAppEvent,
  JsTransportCallbacks,
  JsHttpClientConfig,
  JsStoreCallbacks,
  CacheConfig,
  WasmWhatsAppClient,
} from "../pkg/whatsapp_rust_bridge.js";

/**
 * The real native-addon surface (the napi binding). Most signatures match the
 * public package types; `initWasmEngine` differs (it takes a flat log dispatch
 * instead of the pino `ILogger`, adapted by the façade below).
 */
interface NativeAddon {
  getWasmMemoryBytes(): number;
  getEnabledFeatures(): EnabledFeatures;
  decryptPollVote(
    enc_payload: Uint8Array,
    enc_iv: Uint8Array,
    message_secret: Uint8Array,
    poll_msg_id: string,
    poll_creator_jid: string,
    voter_jid: string,
    option_names: string[],
  ): string[];
  getAggregateVotesInPollMessage(
    option_names: string[],
    voters: PollVoterEntry[],
    message_secret: Uint8Array,
    poll_msg_id: string,
    poll_creator_jid: string,
  ): PollAggregateResult[];
  initWasmEngine(
    logDispatch?: (level: number, target: string, msg: string) => void,
    level?: string,
  ): void;
  createWhatsAppClient(
    transport: JsTransportCallbacks,
    httpClient: JsHttpClientConfig,
    onEvent?: ((event: WhatsAppEvent) => void) | null,
    store?: JsStoreCallbacks | null,
    cache?: CacheConfig | null,
    version?: readonly [number, number, number] | null,
  ): Promise<WasmWhatsAppClient>;
}

// Native napi-rs addon (replaces the old WASM module). The bundled output lives
// at `dist/index.js`; the platform binary is copied next to it as `bridge.node`
// and loaded via `createRequire` (ESM can't `import` a `.node` directly).
const require = createRequire(import.meta.url);
const addon = require("./bridge.node") as NativeAddon;

// Runtime exports from the native addon.
export const getWasmMemoryBytes = addon.getWasmMemoryBytes;
export const getEnabledFeatures = addon.getEnabledFeatures;
export const decryptPollVote = addon.decryptPollVote;
export const getAggregateVotesInPollMessage = addon.getAggregateVotesInPollMessage;
export const createWhatsAppClient = addon.createWhatsAppClient;

// Pure-JS proto codec (bundled; zero runtime deps for consumers, engine-agnostic).
export { encodeProto, decodeProto } from "./proto";

// Auto-assembled protobufjs-style namespace covering every ts-proto type.
export { proto } from "./proto-namespace";

// `log::Level` discriminant → pino method name (Error=1 … Trace=5).
const LOG_LEVELS: Record<number, "error" | "warn" | "info" | "debug" | "trace"> = {
  1: "error",
  2: "warn",
  3: "info",
  4: "debug",
  5: "trace",
};

/**
 * Engine init. Adapts the pino-style `ILogger` into the native addon's flat
 * `(level, target, msg)` dispatch + level string. The `crypto` arg is accepted
 * for API parity and ignored (native uses RustCrypto directly).
 */
export const initWasmEngine = (logger?: any, _crypto?: any): void => {
  if (logger) {
    const dispatch = (level: number, target: string, msg: string): void => {
      const method = LOG_LEVELS[level] ?? "info";
      try {
        logger[method]?.({ target }, msg);
      } catch {
        // never let a logging failure crash the worker thread
      }
    };
    addon.initWasmEngine(dispatch, typeof logger.level === "string" ? logger.level : undefined);
  } else {
    addon.initWasmEngine(undefined, undefined);
  }
};

// All TS types carry over from the (engine-agnostic) generated type surface.
export type * from "../pkg/whatsapp_rust_bridge.js";
