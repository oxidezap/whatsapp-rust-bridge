/**
 * Default relay media channel for encoded-audio calls, behind
 * `setRelayTransportProvider`.
 *
 * WhatsApp relays speak SCTP-over-DTLS-over-UDP with a pre-negotiated
 * id=0 DataChannel, the same channel the native stack builds by hand. A
 * browser reaches the same relay with an `RTCPeerConnection`: this answers
 * it with a synthetic SDP description built from the address plus the ICE
 * credentials the call named, then opens the pre-negotiated channel the
 * relay expects (`ordered: false, maxRetransmits: 0`).
 *
 * The one input the call never names is the relay's DTLS certificate
 * fingerprint. The browser verifies the relay certificate against the
 * `a=fingerprint` line and aborts the handshake on mismatch, so the
 * fingerprint has to be observed once against a live relay and passed in
 * here. It is unproven in this tree: no test here has completed a live
 * relay handshake, and the first one that does gets to confirm or correct
 * the value. Everything else in this file is pinned by unit tests over the
 * exact SDP text.
 */

// Through the entry point, not `../pkg/`: the published `dist/` is
// self-contained and a `../pkg/` specifier in an emitted declaration fails
// the finalize gate. The entry re-exports the whole generated surface.
import type {
  JsRelayConnectionEvents,
  JsRelayConnectionHandle,
  JsRelayConnectionParams,
  JsRelayProviderCallbacks,
} from "./index.js";

// Minimal WebRTC surface: exactly what the provider below touches. Kept as
// module-local shapes because the build targets node consumers and carries
// no DOM lib, and a global `RTCPeerConnection` declaration collides with
// the DOM lib wherever both load. The constructor is read off `globalThis`
// at call time, so a runtime without WebRTC still fails with the named
// error below rather than at import.
interface RtcDataChannelOptions {
  negotiated?: boolean;
  id?: number;
  ordered?: boolean;
  maxRetransmits?: number;
}

interface RtcDataChannel {
  binaryType: string;
  onmessage: ((event: { data: unknown }) => void) | null;
  onopen: (() => void) | null;
  onclose: (() => void) | null;
  onerror: ((event: { message?: unknown }) => void) | null;
  readonly readyState: string;
  send(data: Uint8Array): void;
  close(): void;
}

interface RtcSessionDescriptionInit {
  type: "offer" | "answer";
  sdp?: string;
}

interface RtcPeerConnection {
  createDataChannel(label: string, options?: RtcDataChannelOptions): RtcDataChannel;
  createOffer(): Promise<RtcSessionDescriptionInit>;
  setLocalDescription(desc: RtcSessionDescriptionInit): Promise<void>;
  setRemoteDescription(desc: RtcSessionDescriptionInit): Promise<void>;
  close(): void;
}

type RtcPeerConnectionConstructor = new () => RtcPeerConnection;

function rtcPeerConnectionConstructor(): RtcPeerConnectionConstructor {
  const ctor = (globalThis as unknown as { RTCPeerConnection?: unknown })
    .RTCPeerConnection;
  if (typeof ctor !== "function") {
    throw new Error(
      "no RTCPeerConnection in this runtime; calls need a WebRTC browser"
    );
  }
  return ctor as RtcPeerConnectionConstructor;
}

/** SCTP association port the relay listens on. */
const SCTP_PORT = 5000;
/** Largest SCTP message the relay accepts. */
const MAX_MESSAGE_SIZE = 65536;
/** Relay-side DTLS role: the native stack handshakes as the client. */
const REMOTE_SETUP = "passive";

/**
 * Normalize a SHA-256 fingerprint to the uppercase colon-separated form SDP
 * carries. Accepts hex with or without colons, in either case.
 */
export function normalizeDtlsFingerprint(fingerprint: string): string {
  const hex = fingerprint.replace(/[^0-9a-fA-F]/g, "").toUpperCase();
  if (hex.length !== 64 || /[^0-9A-F]/.test(hex)) {
    throw new Error(
      "DTLS fingerprint must be 32 bytes of hex (a SHA-256 fingerprint)"
    );
  }
  const pairs: string[] = [];
  for (let i = 0; i < hex.length; i += 2) {
    pairs.push(hex.slice(i, i + 2));
  }
  return pairs.join(":");
}

export interface RelayAnswerParts {
  ip: string;
  port: number;
  iceUfrag: string;
  icePwd: string;
  fingerprint: string;
}

/**
 * The synthetic SDP answer describing the relay, byte for byte. The peer
 * connection treats it as the remote side: ICE checks go to `ip:port`
 * signed with `icePwd`, and DTLS verifies against `fingerprint`.
 */
export function buildRelayAnswerSdp(parts: RelayAnswerParts): string {
  const fingerprint = normalizeDtlsFingerprint(parts.fingerprint);
  return [
    "v=0",
    "o=- 0 0 IN IP4 0.0.0.0",
    "s=-",
    "t=0 0",
    `m=application ${parts.port} UDP/DTLS/SCTP webrtc-datachannel`,
    `c=IN IP4 ${parts.ip}`,
    `a=ice-ufrag:${parts.iceUfrag}`,
    `a=ice-pwd:${parts.icePwd}`,
    `a=fingerprint:sha-256:${fingerprint}`,
    `a=setup:${REMOTE_SETUP}`,
    "a=mid:0",
    `a=sctp-port:${SCTP_PORT}`,
    `a=max-message-size:${MAX_MESSAGE_SIZE}`,
    `a=candidate:1 1 udp 2113937151 ${parts.ip} ${parts.port} typ host`,
    "",
  ].join("\r\n");
}

/**
 * Build the default provider: one `RTCPeerConnection` per relay endpoint.
 * Pass the relay's DTLS SHA-256 fingerprint, observed once against a live
 * relay; see the file header for why it cannot come from the call.
 */
export function createRtcRelayTransportProvider(
  dtlsFingerprint: string
): JsRelayProviderCallbacks {
  // Fail at install time, not on the first ring: a malformed fingerprint
  // can never complete a handshake, so keeping it is just a slower error.
  const fingerprint = normalizeDtlsFingerprint(dtlsFingerprint);

  return {
    async createRelayConnection(
      params: JsRelayConnectionParams,
      events: JsRelayConnectionEvents
    ): Promise<JsRelayConnectionHandle> {
      const pc = new (rtcPeerConnectionConstructor())();
      const channel = pc.createDataChannel("pre-negotiated", {
        negotiated: true,
        id: 0,
        ordered: false,
        maxRetransmits: 0,
      });
      channel.binaryType = "arraybuffer";
      channel.onmessage = (event) => {
        events.onPacket(new Uint8Array(event.data as ArrayBuffer));
      };
      channel.onopen = () => {
        events.onOpen();
      };
      const closed = (reason?: string) => {
        events.onClose(reason);
      };
      channel.onclose = () => {
        closed();
      };
      channel.onerror = (event) => {
        closed(typeof event.message === "string" ? event.message : undefined);
      };

      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      await pc.setRemoteDescription({
        type: "answer",
        sdp: buildRelayAnswerSdp({
          ip: params.address,
          port: params.port,
          iceUfrag: params.iceUfrag,
          icePwd: params.icePwd,
          fingerprint,
        }),
      });

      return {
        send(data: Uint8Array) {
          if (channel.readyState !== "open") {
            throw new Error(
              `relay DataChannel is ${channel.readyState}, not open`
            );
          }
          channel.send(data);
        },
        close() {
          try {
            channel.close();
          } finally {
            pc.close();
          }
        },
      };
    },
  };
}
