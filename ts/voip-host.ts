/** Explicit, host-supplied initialization for an isolated VoIP engine WASM. */
import { createVoipBindings } from "../pkg-voip/isolated.js";
import type { SyncInitInput } from "../pkg-voip/whatsapp_rust_voip.js";
import type { VoipBackendCallbacks } from "./voip-backend";
import type { VoipRelayTransport } from "./voip-relay-transport";

type Bindings = ReturnType<typeof createVoipBindings>;
let codecBindings: Bindings | undefined;
const codec = (): Bindings => {
  if (!codecBindings) throw new Error("loadVoip or initVoipSync before using the VoIP codec");
  return codecBindings;
};

// Standalone codec helpers bind to the first initialized engine. Unlike a
// call, a decoder owns only its own native state, never another client's
// transport, session table, or push handler.
export class MlowAudioDecoder {
  private readonly inner: InstanceType<Bindings["MlowAudioDecoder"]>;
  constructor() { this.inner = new (codec().MlowAudioDecoder)(); }
  decode(packet: Uint8Array, payloadType?: number | null): Float32Array {
    return this.inner.decode(packet, payloadType);
  }
  reset(): void { this.inner.reset(); }
  free(): void { this.inner.free(); }
  [Symbol.dispose](): void { this.free(); }
}

export function packetizeOpusForMlow(data: Uint8Array): Uint8Array {
  return codec().packetizeOpusForMlow(data);
}

export function depacketizeOpusFromMlow(data: Uint8Array): Uint8Array {
  return codec().depacketizeOpusFromMlow(data);
}

/** Pass `voipBackend` as the last `createWhatsAppClient` argument's `extensions.voipBackend`. */
export interface VoipEngine {
  voipBackend: VoipBackendCallbacks;
}

/**
 * Initialize a fresh engine with host-provided bytes or a compiled Module.
 * Each call owns a separate WASM instance and relay pipe, even in one isolate.
 * Nothing in the core entrypoint imports or allocates the media engine.
 */
export function initVoipSync(wasm: SyncInitInput, transport: VoipRelayTransport): VoipEngine {
  const engine = createVoipBindings();
  engine.initSync({ module: wasm });
  engine.init(1, 0); // Fail immediately when the engine's OZVP major changes.
  engine.set_relay_transport(transport);
  codecBindings ??= engine;
  return {
    voipBackend: {
      sendFrame: async (frame) => engine.send_frame(frame),
      setPushHandler: (handler) => engine.set_push_handler(handler),
    },
  };
}
