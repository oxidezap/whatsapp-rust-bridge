/** Explicit, host-supplied initialization for the separate VoIP engine WASM. */
import {
  initSync,
  init,
  send_frame,
  set_push_handler,
  set_relay_transport,
  MlowAudioDecoder,
  packetizeOpusForMlow,
  depacketizeOpusFromMlow,
} from "../pkg-voip/whatsapp_rust_voip.js";
import type { SyncInitInput } from "../pkg-voip/whatsapp_rust_voip.js";
import type { VoipBackendCallbacks } from "./voip-backend";
import type { VoipRelayTransport } from "./voip-relay-transport";

export { MlowAudioDecoder, packetizeOpusForMlow, depacketizeOpusFromMlow };

/** Pass `voipBackend` as the last `createWhatsAppClient` argument's `extensions.voipBackend`. */
export interface VoipEngine {
  voipBackend: VoipBackendCallbacks;
}

/**
 * Initialize the engine with host-provided bytes or a compiled Module.
 * Install the host's relay pipe before any call starts. Nothing in the core
 * entrypoint imports this module or allocates the engine's linear memory.
 */
export function initVoipSync(wasm: SyncInitInput, transport: VoipRelayTransport): VoipEngine {
  initSync({ module: wasm });
  init(1, 0); // Fail immediately when the engine's OZVP major changes.
  set_relay_transport(transport);
  return {
    voipBackend: {
      sendFrame: async (frame) => send_frame(frame),
      setPushHandler: (handler) => set_push_handler(handler),
    },
  };
}
