/**
 * Type declaration for the `./wasm` export subpath
 * (`dist/whatsapp_rust_bridge_bg.wasm`).
 *
 * A `.wasm` binary has no per-host type story — workerd delivers a compiled
 * `WebAssembly.Module`, other hosts deliver bytes — so this declares the one
 * thing every host's value has in common: it is exactly wasm-bindgen's
 * `SyncInitInput`, the input `initSync` from
 * `@oxidezap/whatsapp-rust-bridge/host` already takes. No platform is named
 * in the contract:
 *
 * ```ts
 * import wasm from "@oxidezap/whatsapp-rust-bridge/wasm";
 * import { initSync } from "@oxidezap/whatsapp-rust-bridge/host";
 * initSync({ module: wasm });
 * ```
 *
 * Hand-written rather than emitted: tsc's emitDeclarationOnly pass does not
 * copy a lone `.d.ts` source, and the content is one declaration.
 */

import type { SyncInitInput } from "./whatsapp_rust_bridge.js";

declare const wasm: SyncInitInput;

export default wasm;
