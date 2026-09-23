/**
 * The relay-transport contract the engine-side plugin (`voip.wasm`) uses.
 *
 * The bridge's core module speaks OZVP frames; the engine behind those
 * frames needs a packet pipe to one WhatsApp relay endpoint. This is that
 * pipe's contract, on the plugin side: the host supplies a
 * `VoipRelayTransport`, the engine dials it per call. It carries opaque
 * STUN/RTP/RTCP datagrams and knows nothing about their framing — the
 * sans-IO `CallEngine` never touches this; the shell pumps inbound packets
 * into `handle_input` and runs `Output::Transmit` via `send`.
 *
 * This is the contract only. No werift, no node-datachannel, no wrtc, no
 * Node implementation lives here: the host chooses the WebRTC stack, and
 * the existing `relay-transport.ts` RTCPeerConnection adapter stays the
 * default for the resident-engine path. A fake in-memory transport proves
 * the contract in `tests/voip-relay-transport.test.ts`.
 */

/** One WhatsApp relay endpoint, as the engine names it. */
export interface VoipRelayEndpoint {
  /** Relay host, as text (IPv4 literal from the `<relay>` block). */
  address: string;
  /** Relay port. */
  port: number;
  /** Synthetic SDP `ice-ufrag`, built from the `<auth_token>`. */
  iceUfrag: string;
  /** Synthetic SDP `ice-pwd`, from the call's credentials. */
  icePwd: string;
}

/** Inbound packets the connection pushes at the engine. */
export interface VoipRelayConnectionEvents {
  /** One packet (STUN/RTP/RTCP) arrived from the relay. */
  onPacket(data: Uint8Array): void;
  /** The channel is open and ready to carry packets. */
  onOpen(): void;
  /** The channel was lost, with the reason if one was reported. */
  onClose(reason?: string): void;
}

/**
 * One live packet pipe to a relay endpoint. A dumb pipe: `send` ships one
 * opaque datagram, `reconnect` replaces the channel with one to a newly
 * selected endpoint, `close` releases it. VoIP is loss tolerant, so an
 * implementation may drop under backpressure rather than block or error.
 */
export interface VoipRelayConnection {
  /** Send one packet to the relay. */
  send(packet: Uint8Array): void | Promise<void>;
  /**
   * Replace this channel with one connected to a newly selected relay
   * endpoint. Implementations that cannot redial reject, and the engine
   * ends the call instead of sending to the retired relay.
   */
  reconnect(endpoint: VoipRelayEndpoint): Promise<void>;
  /** Close the channel. */
  close(): void | Promise<void>;
}

/**
 * Dials one relay endpoint and hands back the live channel plus its push
 * stream of inbound packets. Mirrors the core's `RelayTransportFactory`:
 * the engine `connect`s, then `send`s outbound packets and pumps inbound
 * ones into the call engine.
 */
export interface VoipRelayTransport {
  /** Connect to the relay and return the channel. */
  connect(
    endpoint: VoipRelayEndpoint,
    events: VoipRelayConnectionEvents,
  ): Promise<VoipRelayConnection>;
}
