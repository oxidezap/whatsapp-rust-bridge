/**
 * Host-loaded entrypoint: same bridge, host-supplied WebAssembly.
 *
 * The default entrypoint (`ts/index.ts`) reads `whatsapp_rust_bridge_bg.wasm`
 * off disk with `node:fs`, which does not exist on some hosts (workerd has no
 * `node:fs` at all; others forbid filesystem reads). This module is identical
 * except for how the wasm arrives: the host imports it with whatever its
 * platform provides and passes it to wasm-bindgen's `initSync`:
 *
 * ```ts
 * // Cloudflare Workers / workerd (static wasm import yields a Module)
 * import wasmModule from "@oxidezap/whatsapp-rust-bridge/wasm";
 * import { initSync } from "@oxidezap/whatsapp-rust-bridge/host";
 * initSync({ module: wasmModule });
 *
 * // Deno (bytes; needs --unstable-raw-imports or a readFile)
 * import wasmBytes from "./whatsapp_rust_bridge_bg.wasm" with { type: "bytes" };
 * import { initSync } from "@oxidezap/whatsapp-rust-bridge/host";
 * initSync({ module: wasmBytes });
 *
 * // Node/Bun without the filesystem read
 * import { readFileSync } from "node:fs";
 * import { initSync } from "@oxidezap/whatsapp-rust-bridge/host";
 * initSync({ module: readFileSync("./whatsapp_rust_bridge_bg.wasm") });
 * ```
 *
 * Why one entrypoint covers every host: `initSync` accepts either a compiled
 * `WebAssembly.Module` or raw bytes (`SyncInitInput`), and the host idioms
 * produce exactly those — workerd/wrangler static imports compile to a
 * `WebAssembly.Module`, Deno/raw-import and bundler `?module` styles produce
 * bytes. There is no workerd-only path because workerd needs nothing this
 * module does not take.
 *
 * Two constraints the host must respect, both from the platform, not the bridge:
 * call `initSync` once per isolate (a second call is a no-op returning the
 * existing instance), and create clients inside a request handler, not at
 * global scope — `crypto.getRandomValues` is unavailable during global-scope
 * evaluation on workerd, so client creation (uuid generation) panics there.
 */

export * from "./surface";
export { initSync } from "../pkg/whatsapp_rust_bridge.js";
