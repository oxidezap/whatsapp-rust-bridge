/**
 * E2E test: WASM client connects to mock server, pairs successfully.
 *
 * Run: NODE_TLS_REJECT_UNAUTHORIZED=0 bun test tests/e2e-connect.test.ts
 */

import { describe, test, expect, beforeAll } from "bun:test";
import {
  initWasmEngine,
  createWhatsAppClient,
  type WhatsAppEvent,
} from "../dist/index.js";
import {
  createTransport,
  createHttp,
  waitForEvent,
  autoScanQr,
  mockServerReachable,
} from "./helpers.js";

process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";

beforeAll(() => {
  initWasmEngine();
});

// The pairing test needs the mock server on MOCK_SERVER_URL, which CI does not
// start. Skipping keeps a missing server out of the build result; a real
// regression still shows up wherever the server is running. Scoped to that test
// so the rest of the suite, which needs no server, keeps running everywhere.
const hasMockServer = await mockServerReachable();

describe("WASM Client E2E", () => {
  test("creates client in disconnected state", async () => {
    const client = await createWhatsAppClient(
      { connect() {}, send() {}, disconnect() {} },
      createHttp()
    );
    expect(client.isConnected()).toBe(false);
    expect(client.isLoggedIn()).toBe(false);
    client.free();
  });

  // `connect()` hands its handshake outcome back from the task that goes on to
  // read the connection. A transport that refuses has to travel that path and
  // reject the promise — an implementation that spawned the connect and
  // returned would resolve here as if the socket had come up.
  test("connect() rejects when the transport refuses", async () => {
    const client = await createWhatsAppClient(
      {
        connect() {
          throw new Error("transport refused");
        },
        send() {},
        disconnect() {},
      },
      createHttp()
    );

    // A client left alive holds background tasks and a slice of the shared
    // WASM heap that the next `createWhatsAppClient` waits on, so a failing
    // assertion here must not become a failure in the test after it.
    try {
      await expect(client.connect()).rejects.toThrow();
      expect(client.isConnected()).toBe(false);
    } finally {
      await client.disconnect();
      client.free();
    }
  }, 20000);

  // The cycle `connect()` now owns: handshake, then a reader that turns frames
  // into events, then teardown. Needs the mock server, like the pairing test
  // below — nothing decodes without a peer that completes the noise handshake.
  // A `connect()` that dropped the core's `Connection` instead of driving it
  // would time out here with no event at all.
  test.skipIf(!hasMockServer)("connect() reads the connection it opened", async () => {
    const events: WhatsAppEvent[] = [];

    // The mock server cannot sign a chain rooted in WhatsApp's issuer, so
    // mock handshakes opt into the testing bypass explicitly. Production
    // code keeps the default.
    const BYPASS_FOR_MOCK = true;
    const client = await createWhatsAppClient(
      createTransport(),
      createHttp(),
      (event) => events.push(event),
      null,
      null,
      null,
      null,
      BYPASS_FOR_MOCK
    );

    try {
      await client.connect();
      expect(client.isConnected()).toBe(true);

      // A `qr` only reaches the host if the reader decoded the server's first
      // stanzas, so it is the cheapest proof that the connection is being read.
      await waitForEvent(events, "qr", 15000);

      await client.disconnect();
      expect(client.isConnected()).toBe(false);
    } finally {
      // `free()` signals shutdown and disconnects on its own, so it covers the
      // paths that never reached the disconnect above.
      client.free();
    }
  }, 25000);

  test.skipIf(!hasMockServer)("connects and pairs with mock server", async () => {
    const events: WhatsAppEvent[] = [];

    // Same mock opt-in as above: the bypass is per client, named at
    // construction, and never a build flag.
    const client = await createWhatsAppClient(
      createTransport(),
      createHttp(),
      (event) => {
        console.log(`  [event] ${event.type}`);
        events.push(event);
      },
      null,
      null,
      null,
      null,
      true
    );

    client.run();

    await Promise.all([autoScanQr(events), waitForEvent(events, "pair_success", 20000)]);

    console.log("  Events:", events.map(e => e.type));
    expect(events.some(e => e.type === "qr")).toBe(true);
    expect(events.some(e => e.type === "pair_success")).toBe(true);

    await client.disconnect();
    client.free();
  }, 25000);
});
