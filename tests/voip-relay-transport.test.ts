/**
 * The plugin-side relay-transport contract (`VoipRelayTransport`): dial,
 * packet flow, and teardown over a fake in-memory transport.
 *
 * No mock server runs in CI and no WebRTC stack is chosen here, so no test
 * here touches a socket. What these prove: a fake transport implementing
 * only the contract delivers inbound packets to the engine's callback,
 * `send` reaches the fake's outbox, `reconnect` swaps the endpoint the
 * fake dials, and `close` releases the channel. The contract is the whole
 * of what the engine side depends on; the fake is the whole of what these
 * tests need.
 */

import { describe, test, expect } from "bun:test";
import type {
  VoipRelayConnection,
  VoipRelayConnectionEvents,
  VoipRelayEndpoint,
  VoipRelayTransport,
} from "../ts/voip-relay-transport";

/** A transport double: records dials, sends, and closes; replays packets. */
function fakeTransport() {
  const dialed: VoipRelayEndpoint[] = [];
  const sent: Uint8Array[][] = [];
  let closes = 0;
  let events: VoipRelayConnectionEvents | null = null;
  let endpoint: VoipRelayEndpoint | null = null;
  const transport: VoipRelayTransport = {
    async connect(
      next: VoipRelayEndpoint,
      nextEvents: VoipRelayConnectionEvents,
    ): Promise<VoipRelayConnection> {
      dialed.push(next);
      events = nextEvents;
      endpoint = next;
      const connection: VoipRelayConnection = {
        send(packet: Uint8Array) {
          sent.push([packet]);
        },
        async reconnect(swapped: VoipRelayEndpoint) {
          dialed.push(swapped);
          endpoint = swapped;
        },
        close() {
          closes += 1;
        },
      };
      return connection;
    },
  };
  return {
    transport,
    dialed,
    sent,
    closes: () => closes,
    events: () => events,
    endpoint: () => endpoint,
    inbound: (packet: Uint8Array) => events?.onPacket(packet),
  };
}

const ENDPOINT: VoipRelayEndpoint = {
  address: "203.0.113.7",
  port: 3478,
  iceUfrag: "UFRAG",
  icePwd: "PWD",
};

describe("VoipRelayTransport contract", () => {
  test("inbound packets reach the engine callback", async () => {
    const fake = fakeTransport();
    const received: Uint8Array[] = [];
    await fake.transport.connect(ENDPOINT, {
      onPacket: (packet) => received.push(packet),
      onOpen: () => {},
      onClose: () => {},
    });
    const first = new Uint8Array([0x80, 0x78, 0x12, 0x34]);
    const second = new Uint8Array([0x00, 0x01, 0x00, 0x00]);
    fake.inbound(first);
    fake.inbound(second);
    expect(received).toEqual([first, second]);
  });

  test("send reaches the transport and close releases the channel", async () => {
    const fake = fakeTransport();
    const connection = await fake.transport.connect(ENDPOINT, {
      onPacket: () => {},
      onOpen: () => {},
      onClose: () => {},
    });
    const packet = new Uint8Array([0x90, 0x78, 0x12, 0x34]);
    await connection.send(packet);
    expect(fake.sent).toEqual([[packet]]);
    expect(fake.closes()).toBe(0);
    await connection.close();
    expect(fake.closes()).toBe(1);
  });

  test("reconnect swaps the dialed endpoint", async () => {
    const fake = fakeTransport();
    const connection = await fake.transport.connect(ENDPOINT, {
      onPacket: () => {},
      onOpen: () => {},
      onClose: () => {},
    });
    expect(fake.dialed).toEqual([ENDPOINT]);
    const swapped: VoipRelayEndpoint = {
      address: "198.51.100.9",
      port: 3479,
      iceUfrag: "UFRAG2",
      icePwd: "PWD2",
    };
    await connection.reconnect(swapped);
    expect(fake.dialed).toEqual([ENDPOINT, swapped]);
    expect(fake.endpoint()).toEqual(swapped);
  });
});
