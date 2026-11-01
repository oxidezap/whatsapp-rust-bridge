/** Node/Bun loader for the separate VoIP engine. Never imported by the core entrypoint. */
import { readFileSync } from "node:fs";
import { initVoipSync } from "./voip-host";
import type { VoipRelayTransport } from "./voip-relay-transport";

export { MlowAudioDecoder, packetizeOpusForMlow, depacketizeOpusFromMlow } from "./voip-host";
export type { VoipEngine } from "./voip-host";

/**
 * Load the packaged voip.wasm on demand. For hosts without node:fs, import
 * `/voip/wasm` using the host's wasm loader and call `/voip/host`'s
 * `initVoipSync` instead. Provide the returned `voipBackend` to the client's
 * trailing `extensions` argument before any calls start.
 */
export function loadVoip(transport: VoipRelayTransport) {
  const bytes = readFileSync(new URL("whatsapp_rust_voip_bg.wasm", import.meta.url));
  return initVoipSync(bytes, transport);
}
