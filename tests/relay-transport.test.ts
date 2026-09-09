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

import { describe, test, expect } from "bun:test";
import {
  buildRelayAnswerSdp,
  createRtcRelayTransportProvider,
  normalizeDtlsFingerprint,
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
        `a=fingerprint:sha-256:${FINGERPRINT}`,
        "a=setup:passive",
        "a=mid:0",
        "a=sctp-port:5000",
        "a=max-message-size:65536",
        "a=candidate:1 1 udp 2113937151 203.0.113.7 3478 typ host",
        "",
      ].join("\r\n")
    );
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
    for (const bad of ["", "AA:BB", FINGERPRINT.slice(0, 63), "ZZ".repeat(32)]) {
      expect(() => normalizeDtlsFingerprint(bad)).toThrow();
    }
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
});
