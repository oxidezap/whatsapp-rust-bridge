import { readFileSync } from "node:fs";
import { initSync } from "../pkg/whatsapp_rust_bridge.js";

const wasmUrl = new URL("whatsapp_rust_bridge_bg.wasm", import.meta.url);
const wasmBytes = readFileSync(wasmUrl);
initSync({ module: wasmBytes });

// The bridge surface lives in `./surface`; this entrypoint only finds and
// loads the wasm, then re-exports it.
export * from "./surface";
