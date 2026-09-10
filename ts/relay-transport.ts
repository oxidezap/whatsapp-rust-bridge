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
  readonly bufferedAmount: number;
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
 * carries. Accepts hex with or without colons, in either case — and nothing
 * else: stripping every non-hex character first would let trailing garbage
 * through, so only colons are removed and the rest must be hex.
 */
export function normalizeDtlsFingerprint(fingerprint: string): string {
  const hex = fingerprint.replace(/:/g, "").toUpperCase();
  if (!/^[0-9A-F]{64}$/.test(hex)) {
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
 * signed with `icePwd`, and DTLS verifies against `fingerprint`. The
 * address family follows the relay literal — an IPv6 relay with an `IP4`
 * connection line is rejected before ICE ever runs.
 */
export function buildRelayAnswerSdp(parts: RelayAnswerParts): string {
  const fingerprint = normalizeDtlsFingerprint(parts.fingerprint);
  const family = parts.ip.includes(":") ? "IP6" : "IP4";
  const unspecified = family === "IP6" ? "::" : "0.0.0.0";
  return [
    "v=0",
    `o=- 0 0 IN ${family} ${unspecified}`,
    "s=-",
    "t=0 0",
    `m=application ${parts.port} UDP/DTLS/SCTP webrtc-datachannel`,
    `c=IN ${family} ${parts.ip}`,
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
 * Decide whether an outbound datagram must shed: the browser queues past
 * `max` bytes of unsent SCTP, and voice tolerates loss but not unbounded
 * native memory. Pure so the policy is pinnable without a peer connection.
 */
export function shedBufferedPacket(bufferedAmount: number, max: number): boolean {
  return bufferedAmount > max;
}

export interface RtcRelayTransportOptions {
  /**
   * Unsent bytes past which outbound datagrams shed instead of queueing.
   * Defaults to 8192, on the order of tens of voice packets; voice is
   * loss tolerant, browser send queues are not bounded.
   */
  maxBufferedAmount?: number;
  /** Called with the shed count, so host-side loss stays observable. */
  onPacketsDropped?: (count: number) => void;
}

/**
 * Build the default provider: one `RTCPeerConnection` per relay endpoint.
 * Pass the relay's DTLS SHA-256 fingerprint, observed once against a live
 * relay; see the file header for why it cannot come from the call.
 */
export function createRtcRelayTransportProvider(
  dtlsFingerprint: string,
  options?: RtcRelayTransportOptions
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

      // Any handshake step can reject — a malformed answer, an
      // unsupported attribute, an internal WebRTC error. The caller never
      // receives a handle on that path, so nothing could close the half-open
      // connection; release it here instead of leaking a peer connection
      // per failed ring. Detaching the handlers first keeps the cleanup
      // from reporting a close the Rust side already learned as a rejection.
      try {
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
      } catch (err) {
        channel.onclose = null;
        channel.onerror = null;
        try {
          channel.close();
        } finally {
          pc.close();
        }
        throw err;
      }

      const maxBuffered = options?.maxBufferedAmount ?? 8192;
      const dropped = options?.onPacketsDropped;
      let shed = 0;
      const reportShed = () => {
        if (shed > 0) {
          const count = shed;
          shed = 0;
          dropped?.(count);
        }
      };

      return {
        send(data: Uint8Array) {
          if (channel.readyState !== "open") {
            throw new Error(
              `relay DataChannel is ${channel.readyState}, not open`
            );
          }
          if (shedBufferedPacket(channel.bufferedAmount, maxBuffered)) {
            shed += 1;
            reportShed();
            return;
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
