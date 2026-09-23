/**
 * Client extensions: the trailing optional argument of `createWhatsAppClient`.
 *
 * The object carries whole plugin families, never one loose knob. Today it
 * carries one: `voipBackend`, the media plugin behind which the engine WASM
 * lives. Absent, no second module loads, compiles, or costs memory.
 */

/**
 * The media plugin contract. The bridge speaks OZVP frames; the plugin
 * answers them.
 *
 * - `sendFrame` takes one request frame and resolves exactly one response
 *   frame. It never receives a notification opcode.
 * - `setPushHandler` receives the bridge's inbound entry point, called once
 *   at install. The plugin calls it for `OPEN`, `EVENT`, `STATS` pushes,
 *   `MEDIA_ENDED`, and the `_OUT` media frames.
 */
export interface VoipBackendCallbacks {
  sendFrame(frame: Uint8Array): Promise<Uint8Array>;
  setPushHandler(handler: (frame: Uint8Array) => void): void;
}

/** Trailing `extensions` argument of `createWhatsAppClient`. */
export interface ClientExtensions {
  voipBackend?: VoipBackendCallbacks | null;
}
