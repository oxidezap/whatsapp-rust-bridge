/**
 * Relay SDP synthesis for encoded-audio calls.
 *
 * The synthetic answer is the whole of what the browser's ICE and DTLS
 * stacks learn about the relay, so its text is pinned byte for byte. A live
 * handshake against a real relay is still unproven in this tree (no account
 * here); what these prove is that the builder emits exactly the description
 * the transport contract names, and that bad inputs fail before a peer
 * connection is ever built.
 */

import { afterEach, describe, test, expect } from "bun:test";
import {
  buildRelayAnswerSdp,
  createRtcRelayTransportProvider,
  evaluateOutboundPacket,
  isRelayControlPacket,
  normalizeDtlsFingerprint,
  OutboundAuTracker,
  RELAY_DTLS_FINGERPRINT,
  RTP_PAYLOAD_TYPE_H264,
  shedBufferedPacket,
} from "../ts/relay-transport";

const FINGERPRINT =
  "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99";

describe("relay answer SDP", () => {
  test("it describes the relay exactly", () => {
    expect(
      buildRelayAnswerSdp({
        ip: "203.0.113.7",
        port: 3478,
        iceUfrag: "UFRAG",
        icePwd: "PWD",
        fingerprint: FINGERPRINT,
      })
    ).toBe(
      [
        "v=0",
        "o=- 0 0 IN IP4 0.0.0.0",
        "s=-",
        "t=0 0",
        "m=application 3478 UDP/DTLS/SCTP webrtc-datachannel",
        "c=IN IP4 203.0.113.7",
        "a=ice-ufrag:UFRAG",
        "a=ice-pwd:PWD",
        `a=fingerprint:sha-256 ${FINGERPRINT}`,
        "a=setup:passive",
        "a=mid:0",
        "a=sctp-port:5000",
        "a=max-message-size:65536",
        "a=candidate:1 1 udp 2113937151 203.0.113.7 3478 typ host",
        "",
      ].join("\r\n")
    );
  });

  test("an IPv6 relay gets an IPv6 connection line", () => {
    const sdp = buildRelayAnswerSdp({
      ip: "2001:db8::7",
      port: 3478,
      iceUfrag: "U",
      icePwd: "P",
      fingerprint: FINGERPRINT,
    });
    expect(sdp).toContain("c=IN IP6 2001:db8::7");
    expect(sdp).toContain("o=- 0 0 IN IP6 ::");
    expect(sdp).not.toContain("IP4");
  });

  test("the relay is the DTLS server", () => {
    // The native stack handshakes as the client, so the answer marks the
    // relay passive; the browser then takes the active role.
    const sdp = buildRelayAnswerSdp({
      ip: "203.0.113.7",
      port: 3478,
      iceUfrag: "U",
      icePwd: "P",
      fingerprint: FINGERPRINT,
    });
    expect(sdp).toContain("a=setup:passive");
  });
});

describe("fingerprint normalization", () => {
  test("it accepts hex in any case, with or without colons", () => {
    const bare = FINGERPRINT.replaceAll(":", "").toLowerCase();
    expect(normalizeDtlsFingerprint(bare)).toBe(FINGERPRINT);
    expect(normalizeDtlsFingerprint(FINGERPRINT.toLowerCase())).toBe(
      FINGERPRINT
    );
  });

  test("it rejects anything that is not 32 bytes of hex", () => {
    const bare = FINGERPRINT.replaceAll(":", "");
    for (const bad of [
      "",
      "AA:BB",
      bare.slice(0, -1),
      `${bare}AA`,
      `${bare}ZZ`,
      "ZZ".repeat(32),
    ]) {
      expect(() => normalizeDtlsFingerprint(bad)).toThrow();
    }
  });
});

describe("send backpressure", () => {
  test("only an over-full buffer sheds", () => {
    expect(shedBufferedPacket(0, 8192)).toBe(false);
    expect(shedBufferedPacket(8192, 8192)).toBe(false);
    expect(shedBufferedPacket(8193, 8192)).toBe(true);
  });
});

type FakeChannel = {
  binaryType: string;
  bufferedAmount: number;
  onmessage: ((event: { data: unknown }) => void) | null;
  onopen: (() => void) | null;
  onclose: (() => void) | null;
  onerror: ((event: { message?: unknown }) => void) | null;
  readyState: string;
  sent: unknown[];
  closed: boolean;
  send(data: unknown): void;
  close(): void;
};

type Script = {
  createDataChannelThrows?: boolean;
  openDelay?: number;
  closeBeforeOpen?: boolean;
  failConnection?: boolean;
  closedPcs: number;
};

function installFakePeerConnection(script: Script) {
  const channels: FakeChannel[] = [];
  (globalThis as Record<string, unknown>).RTCPeerConnection =
    class {
      closed = false;
      connectionState = "new";
      onconnectionstatechange: (() => void) | null = null;
      createDataChannel() {
        if (script.createDataChannelThrows) {
          throw new Error("negotiated channels unsupported");
        }
        const channel: FakeChannel = {
          binaryType: "",
          bufferedAmount: 0,
          onmessage: null,
          onopen: null,
          onclose: null,
          onerror: null,
          readyState: "connecting",
          sent: [],
          closed: false,
          send(data: unknown) {
            if (channel.readyState !== "open") throw new Error("not open");
            channel.sent.push(data);
          },
          close() {
            channel.closed = true;
          },
        };
        channels.push(channel);
        if (script.closeBeforeOpen) {
          queueMicrotask(() => {
            channel.readyState = "closed";
            channel.onclose?.();
          });
        } else {
          const delay = script.openDelay ?? 0;
          setTimeout(() => {
            channel.readyState = "open";
            channel.onopen?.();
          }, delay);
        }
        return channel;
      }
      async createOffer() {
        return { type: "offer", sdp: "" };
      }
      async setLocalDescription() {}
      async setRemoteDescription() {
        if (script.failConnection) {
          const self = this as {
            connectionState: string;
            onconnectionstatechange: (() => void) | null;
          };
          queueMicrotask(() => {
            self.connectionState = "failed";
            self.onconnectionstatechange?.();
          });
        }
      }
      close() {
        this.closed = true;
        script.closedPcs += 1;
      }
    };
  return channels;
}

afterEach(() => {
  delete (globalThis as Record<string, unknown>).RTCPeerConnection;
});

describe("handshake lifecycle", () => {

  test("the handle resolves only once the channel carries datagrams", async () => {
    const script: Script = { openDelay: 5, closedPcs: 0 };
    installFakePeerConnection(script);
    const events: string[] = [];
    const provider = createRtcRelayTransportProvider(FINGERPRINT);
    const handle = await provider.createRelayConnection(
      { address: "203.0.113.7", port: 3478, iceUfrag: "U", icePwd: "P" },
      {
        onPacket() {},
        onOpen() {
          events.push("open");
        },
        onClose() {},
      }
    );
    // Open fired as part of construction, before the handle came back.
    expect(events).toEqual(["open"]);
    expect(script.closedPcs).toBe(0);
    handle.send(new Uint8Array([1]));
  });

  test("a throwing channel creation releases the peer connection", async () => {
    const script: Script = { createDataChannelThrows: true, closedPcs: 0 };
    installFakePeerConnection(script);
    const provider = createRtcRelayTransportProvider(FINGERPRINT);
    await expect(
      provider.createRelayConnection(
        { address: "203.0.113.7", port: 3478, iceUfrag: "U", icePwd: "P" },
        { onPacket() {}, onOpen() {}, onClose() {} }
      )
    ).rejects.toThrow(/negotiated channels unsupported/);
    expect(script.closedPcs).toBe(1);
  });

  test("a failed connection rejects and releases the peer connection", async () => {
    const script: Script = { failConnection: true, closedPcs: 0 };
    installFakePeerConnection(script);
    const provider = createRtcRelayTransportProvider(FINGERPRINT);
    await expect(
      provider.createRelayConnection(
        { address: "203.0.113.7", port: 3478, iceUfrag: "U", icePwd: "P" },
        { onPacket() {}, onOpen() {}, onClose() {} }
      )
    ).rejects.toThrow(/peer connection failed/);
    expect(script.closedPcs).toBe(1);
  });

  test("a close before open rejects instead of hanging", async () => {
    const script: Script = { closeBeforeOpen: true, closedPcs: 0 };
    installFakePeerConnection(script);
    const provider = createRtcRelayTransportProvider(FINGERPRINT);
    await expect(
      provider.createRelayConnection(
        { address: "203.0.113.7", port: 3478, iceUfrag: "U", icePwd: "P" },
        { onPacket() {}, onOpen() {}, onClose() {} }
      )
    ).rejects.toThrow(/closed before opening/);
    expect(script.closedPcs).toBe(1);
  });
});

describe("send path", () => {
  async function openHandle(
    options?: Parameters<typeof createRtcRelayTransportProvider>[1]
  ) {
    const script: Script = { openDelay: 0, closedPcs: 0 };
    const channels = installFakePeerConnection(script);
    const provider = createRtcRelayTransportProvider(FINGERPRINT, options);
    const handle = await provider.createRelayConnection(
      { address: "203.0.113.7", port: 3478, iceUfrag: "U", icePwd: "P" },
      { onPacket() {}, onOpen() {}, onClose() {} }
    );
    return { handle, channel: channels[0]! };
  }

  test("a drained channel sends", async () => {
    const { handle, channel } = await openHandle();
    channel.bufferedAmount = 0;
    handle.send(new Uint8Array([1, 2, 3]));
    expect(channel.sent.length).toBe(1);
  });

  test("a stalled channel sheds and reports the count", async () => {
    const dropped: number[] = [];
    const { handle, channel } = await openHandle({
      maxBufferedAmount: 8,
      onPacketsDropped: (count) => dropped.push(count),
    });
    channel.bufferedAmount = 1024;
    handle.send(new Uint8Array([1]));
    handle.send(new Uint8Array([2]));
    expect(channel.sent.length).toBe(0);
    expect(dropped).toEqual([1, 1]);
  });

  test("a video access unit is sent completely even if buffer exceeds ceiling during burst", async () => {
    const dropped: number[] = [];
    const { handle, channel } = await openHandle({
      maxBufferedAmount: 100,
      onPacketsDropped: (count) => dropped.push(count),
    });
    channel.bufferedAmount = 0;

    const frag1 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 1]);
    const frag2 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 2]);
    const frag3 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264 | 0x80, 0, 3]); // marker = 1

    handle.send(frag1);
    // Buffer grows past 100 during send
    channel.bufferedAmount = 500;
    handle.send(frag2);
    channel.bufferedAmount = 600;
    handle.send(frag3);

    expect(channel.sent.length).toBe(3);
    expect(dropped.length).toBe(0);
  });

  test("control packets pass unconditionally even when buffer is saturated", async () => {
    const { handle, channel } = await openHandle({ maxBufferedAmount: 8 });
    channel.bufferedAmount = 65536;

    const stun = new Uint8Array([0x00, 0x01, 0, 0]);
    const rtcp = new Uint8Array([0x80, 206, 0, 0, 0, 0, 0, 0]); // PLI feedback

    handle.send(stun);
    handle.send(rtcp);

    expect(channel.sent.length).toBe(2);
  });
});

describe("outbound packet admission and AU awareness", () => {
  test("identifies STUN and RTCP control packets", () => {
    expect(isRelayControlPacket(new Uint8Array([0x00, 0x01, 0, 0]))).toBe(true);
    expect(isRelayControlPacket(new Uint8Array([0x01, 0x01, 0, 0]))).toBe(true);
    expect(isRelayControlPacket(new Uint8Array([0x80, 200, 0, 0, 0, 0, 0, 0]))).toBe(true);
    expect(isRelayControlPacket(new Uint8Array([0x80, 201, 0, 0, 0, 0, 0, 0]))).toBe(true);
    expect(isRelayControlPacket(new Uint8Array([0x80, 206, 0, 0, 0, 0, 0, 0]))).toBe(true);
    expect(isRelayControlPacket(new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 0, 0, 0, 0, 0]))).toBe(false);
    expect(isRelayControlPacket(new Uint8Array([0x80]))).toBe(false);
  });

  test("control packets are never dropped even when buffer is huge", () => {
    const stun = new Uint8Array([0x00, 0x01, 0, 0]);
    const rtcp = new Uint8Array([0x80, 206, 0, 0, 0, 0, 0, 0]);
    expect(evaluateOutboundPacket(stun, 10_000_000, "between", 65536).shouldSend).toBe(true);
    expect(evaluateOutboundPacket(rtcp, 10_000_000, "between", 65536).shouldSend).toBe(true);
  });

  test("video access units are sent to completion once begun", () => {
    const frag1 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 1]);
    const frag2 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 2]);
    const frag3 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264 | 0x80, 0, 3]);

    const v1 = evaluateOutboundPacket(frag1, 1000, "between", 65536);
    expect(v1.shouldSend).toBe(true);
    expect(v1.nextState).toBe("send");

    const v2 = evaluateOutboundPacket(frag2, 100_000, v1.nextState, 65536);
    expect(v2.shouldSend).toBe(true);
    expect(v2.nextState).toBe("send");

    const v3 = evaluateOutboundPacket(frag3, 120_000, v2.nextState, 65536);
    expect(v3.shouldSend).toBe(true);
    expect(v3.nextState).toBe("between");
  });

  test("video access units are dropped entirely if over ceiling at start", () => {
    const frag1 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 1]);
    const frag2 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 2]);
    const frag3 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264 | 0x80, 0, 3]);

    const v1 = evaluateOutboundPacket(frag1, 70_000, "between", 65536);
    expect(v1.shouldSend).toBe(false);
    expect(v1.nextState).toBe("drop");

    const v2 = evaluateOutboundPacket(frag2, 1000, v1.nextState, 65536);
    expect(v2.shouldSend).toBe(false);
    expect(v2.nextState).toBe("drop");

    const v3 = evaluateOutboundPacket(frag3, 1000, v2.nextState, 65536);
    expect(v3.shouldSend).toBe(false);
    expect(v3.nextState).toBe("between");

    const nextAuFrag1 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 4]);
    const v4 = evaluateOutboundPacket(nextAuFrag1, 1000, v3.nextState, 65536);
    expect(v4.shouldSend).toBe(true);
    expect(v4.nextState).toBe("send");
  });

  test("audio packets are exempt from soft ceiling but drop when wedged", () => {
    const opus = new Uint8Array([0x80, 102, 0, 1]);
    expect(evaluateOutboundPacket(opus, 100_000, "between", 65536).shouldSend).toBe(true);
    expect(evaluateOutboundPacket(opus, 600_000, "between", 65536).shouldSend).toBe(false);
  });

  test("OutboundAuTracker manages access unit state across packets without allocation", () => {
    const tracker = new OutboundAuTracker(65536, 524288);
    expect(tracker.state).toBe("between");

    const frag1 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 1]);
    const frag2 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264, 0, 2]);
    const frag3 = new Uint8Array([0x80, RTP_PAYLOAD_TYPE_H264 | 0x80, 0, 3]);

    expect(tracker.shouldSend(frag1, 1000)).toBe(true);
    expect(tracker.state).toBe("send");
    expect(tracker.shouldSend(frag2, 100_000)).toBe(true);
    expect(tracker.state).toBe("send");
    expect(tracker.shouldSend(frag3, 120_000)).toBe(true);
    expect(tracker.state).toBe("between");

    // Next AU admitted when over ceiling is dropped to completion
    expect(tracker.shouldSend(frag1, 70_000)).toBe(false);
    expect(tracker.state).toBe("drop");
    expect(tracker.shouldSend(frag2, 1000)).toBe(false);
    expect(tracker.state).toBe("drop");
    expect(tracker.shouldSend(frag3, 1000)).toBe(false);
    expect(tracker.state).toBe("between");

    // Reset restores between state
    tracker.state = "drop";
    tracker.reset();
    expect(tracker.state).toBe("between");
  });
});

describe("provider construction", () => {
  test("a bad fingerprint fails at install time, not on the first ring", () => {
    expect(() => createRtcRelayTransportProvider("not-hex")).toThrow();
  });

  test("without WebRTC the constructor rejects instead of hanging", async () => {
    const provider = createRtcRelayTransportProvider(FINGERPRINT);
    const events = { onPacket() {}, onOpen() {}, onClose() {} };
    await expect(
      provider.createRelayConnection(
        { address: "203.0.113.7", port: 3478, iceUfrag: "U", icePwd: "P" },
        events
      )
    ).rejects.toThrow(/RTCPeerConnection/);
  });

  test("uses RELAY_DTLS_FINGERPRINT by default when fingerprint is omitted", () => {
    expect(RELAY_DTLS_FINGERPRINT).toMatch(/^([0-9A-F]{2}:){31}[0-9A-F]{2}$/);
    expect(() => createRtcRelayTransportProvider()).not.toThrow();
  });

  test("uses RTCPeerConnection passed in options", async () => {
    let constructed = false;
    class CustomPC {
      constructor() {
        constructed = true;
        throw new Error("custom PC used");
      }
    }
    const provider = createRtcRelayTransportProvider(undefined, {
      RTCPeerConnection: CustomPC,
    });
    const events = { onPacket() {}, onOpen() {}, onClose() {} };
    await expect(
      provider.createRelayConnection(
        { address: "203.0.113.7", port: 3478, iceUfrag: "U", icePwd: "P" },
        events
      )
    ).rejects.toThrow("custom PC used");
    expect(constructed).toBe(true);
  });
});
