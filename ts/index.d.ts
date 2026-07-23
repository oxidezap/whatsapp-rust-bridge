export * from "../pkg/whatsapp_rust_bridge.js";
export { encodeProto, decodeProto } from "./proto";
export { proto } from "./proto-namespace";
import type { WhatsAppEventHandler, JsTransportCallbacks, JsHttpClientConfig, JsStoreCallbacks, CacheConfig } from "../pkg/whatsapp_rust_bridge.js";
import type { WasmWhatsAppClient } from "../pkg/whatsapp_rust_bridge.js";
export declare const initWasmEngine: (logger?: any, crypto?: any) => void;
export declare const createWhatsAppClient: (transport: JsTransportCallbacks, httpClient: JsHttpClientConfig, onEvent?: WhatsAppEventHandler | null, store?: JsStoreCallbacks | null, cache?: CacheConfig | null, version?: readonly [number, number, number] | null, wantedPreKeyCount?: number | null) => Promise<WasmWhatsAppClient>;
