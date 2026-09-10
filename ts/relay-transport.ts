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
  readonly connectionState: string;
  onconnectionstatechange: (() => void) | null;
  createDataChannel(label: string, options?: RtcDataChannelOptions): RtcDataChannel;
  createOffer(): Promise<RtcSessionDescriptionInit>;
  setLocalDescription(desc: RtcSessionDescriptionInit): Promise<void>;
  setRemoteDescription(desc: RtcSessionDescriptionInit): Promise<void>;
  close(): void;
}

/**
 * Upper bound on the channel-open wait. Past the core's 15s provider
 * timeout on purpose: the Rust side gives up first, and this only fires
 * when even that path never ran (a stalled ICE agent that emits no
 * DataChannel event at all), releasing peer connections no handle could
 * ever close.
 */
const OPEN_TIMEOUT_MS = 20000;

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
      // Everything from channel creation on lives inside the guarded
      // region below: a throwing createDataChannel, a rejected offer or
      // answer, or a channel that closes before opening must all release
      // the peer connection, since the caller never receives a handle to
      // close it with. Each failed ring would otherwise leak a peer
      // connection with its ICE agent and sockets.
      let channel: RtcDataChannel | undefined;
      const release = () => {
        if (channel !== undefined) {
          channel.onclose = null;
          channel.onerror = null;
          try {
            channel.close();
          } catch {
            // Already gone; the peer connection close below is the part
            // that matters.
          }
        }
        pc.close();
      };
      const openHooks: { resolve?: () => void; reject?: (err: Error) => void } =
        {};
      const opened = new Promise<void>((resolve, reject) => {
        openHooks.resolve = resolve;
        openHooks.reject = reject;
      });
      const settleOpen = (fn: () => void) => {
        clearTimeout(openTimer);
        pc.onconnectionstatechange = null;
        fn();
      };
      const openTimer = setTimeout(() => {
        settleOpen(() =>
          openHooks.reject?.(
            new Error("relay DataChannel did not open within 20s")
          )
        );
      }, OPEN_TIMEOUT_MS);
      pc.onconnectionstatechange = () => {
        // A terminal ICE state with no DataChannel event: stop waiting
        // now rather than holding the peer connection to the timeout.
        if (pc.connectionState === "failed" || pc.connectionState === "closed") {
          const state = pc.connectionState;
          settleOpen(() =>
            openHooks.reject?.(new Error(`relay peer connection ${state}`))
          );
        }
      };
      try {
        channel = pc.createDataChannel("pre-negotiated", {
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
          settleOpen(() => openHooks.resolve?.());
        };
        const closed = (reason?: string) => {
          events.onClose(reason);
        };
        channel.onclose = () => {
          closed();
          settleOpen(() =>
            openHooks.reject?.(new Error("relay DataChannel closed before opening"))
          );
        };
        channel.onerror = (event) => {
          const reason =
            typeof event.message === "string" ? event.message : undefined;
          closed(reason);
          settleOpen(() =>
            openHooks.reject?.(new Error(reason ?? "relay DataChannel errored"))
          );
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
        // setRemoteDescription resolves while the channel is usually still
        // connecting; returning here would hand back a handle whose send
        // throws until onopen. Resolve the construction only once the
        // channel carries datagrams, and reject on an intervening close.
        await opened;
      } catch (err) {
        release();
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
